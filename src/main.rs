use clap::Parser;

mod broker;
mod config;
mod mqtt;
mod png_encode;
mod protocol;
mod ws;

mod terminal;

mod ui;

pub use vibetty_screenshot as screenshot;

use config::{Cli, Commands};

mod setup;
mod static_page;

fn logger_init() -> anyhow::Result<flexi_logger::LoggerHandle> {
    use flexi_logger::{FileSpec, Logger, WriteMode};

    let logger = Logger::try_with_env_or_str("info")?
        .log_to_file(FileSpec::default().suppress_timestamp())
        .append() // 每次启动接着上次写;达到 rotate 的 10MB 才轮转成新文件
        .write_mode(WriteMode::BufferAndFlush)
        .rotate(
            flexi_logger::Criterion::Size(10_000_000), // 10MB
            flexi_logger::Naming::Numbers,
            flexi_logger::Cleanup::KeepForDays(5),
        )
        .format_for_files(flexi_logger::detailed_format)
        .start()?;

    Ok(logger)
}

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    let _logger = logger_init().expect("Failed to initialize logger");

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Setup) => {
            if let Err(e) = setup::run_setup(cli.config.clone()) {
                eprintln!("Setup error: {e}");
                std::process::exit(1);
            }
            return;
        }
        None => {}
    }

    // Default: Run mode
    let args = cli.run_args();

    if args.command.is_empty() {
        eprintln!("Error: No command specified. Use -- to separate options and command.");
        std::process::exit(1);
    }

    log::info!("Starting Vibetty with command: {:?}", args.command);

    let (cli_tx, cli_rx) = tokio::sync::mpsc::channel(100);
    let (tx, rx) = tokio::sync::broadcast::channel(100);
    drop(rx);

    let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(100);

    let (screenshot_tx, screenshot_rx) = tokio::sync::mpsc::channel(4);

    let image_format = args.image_format();

    // 可选 MQTT 传输:配置里有 [mqtt] 段且 enable!=false 才启用,否则完全不碰。
    match args.mqtt_config() {
        Some(mut cfg) if cfg.enable => {
            if cfg.builtin_broker {
                // 内置 broker:进程内起 rumqttd(TCP port + WS builtin_ws_port,匿名),
                // 自身 client 改连本地;ESP32 直接连本机 port。host/use_tls 被忽略。
                match broker::spawn_builtin(&cfg) {
                    Ok(()) => {
                        cfg.broker = format!("mqtt://127.0.0.1:{}", cfg.builtin_port);
                        log::info!(
                            "[mqtt] builtin broker on :{}(tcp) + :{}(ws); client connects locally",
                            cfg.builtin_port,
                            cfg.builtin_ws_port
                        );
                    }
                    Err(e) => log::error!("[mqtt] failed to start builtin broker: {e}"),
                }
            }
            mqtt::spawn(cfg, cli_tx.clone(), tx.clone(), image_format);
            log::info!("[mqtt] transport enabled");
        }
        Some(_) => log::info!("[mqtt] transport disabled by config (enable=false)"),
        None => log::debug!("[mqtt] not configured, transport disabled"),
    }

    // HTTP server 默认不启动;由 TUI footer 按钮按需开启(见 ws::run_command)。
    // --bind-addr 整体(如 `0.0.0.0:3000`)作为对话框预填默认值;其端口部分注入 VIBETTY_PORT。
    let default_bind = args.bind_addr.clone();

    // Init TUI
    let mut tui = ui::init_terminal().expect("Failed to initialize terminal");
    ui::spawn_event_loop(ui_tx);

    let mut ui_title = String::new();
    let mut ui_rx = ui_rx;

    let command = args.command;
    if let Err(e) = ws::run_command(
        command,
        cli_rx,
        &mut ui_rx,
        tx.clone(),
        screenshot_tx,
        default_bind,
        screenshot_rx,
        &mut tui,
        &mut ui_title,
        image_format,
        args.auto_submit,
    )
    .await
    {
        log::error!("Error in command execution: {}", e);
    }

    ui::cleanup_terminal(&mut tui).ok();
}
