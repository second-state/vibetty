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

    // 可选 MQTT:有 [mqtt] 段就把整段配置交给 run_command。
    // boot 自动起(enable→client、builtin_broker→broker)+ footer 按钮起停都在 run_command 里;
    // enable / builtin_broker 现在纯粹当 auto-start 标志(不再用来藏按钮或必起 transport)。
    let mqtt_cfg = args.mqtt_config();

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
        cli_tx,
        screenshot_tx,
        default_bind,
        mqtt_cfg,
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
