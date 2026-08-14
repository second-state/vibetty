use clap::Parser;

mod broker;
mod config;
mod herdr_runner;
mod mqtt;
#[allow(dead_code)] // PNG 编码已不再走 render(只 JPEG);模块保留待后续清理
mod png_encode;
mod protocol;
mod ws;

mod terminal;

mod ui;

pub use vibetty_screenshot as screenshot;

use config::{Cli, Commands};

mod setup;
mod skill;
mod static_page;

/// 空操作 tracing subscriber:吞掉所有 tracing 事件。用于屏蔽 rumqttd 的 tracing 日志
/// (rumqttd 用 tracing,不是 log)。见 `logger_init`。
mod null_subscriber {
    use std::sync::atomic::{AtomicU64, Ordering};
    use tracing::{Event, Metadata, Subscriber, span};

    pub struct Null {
        next: AtomicU64,
    }

    impl Null {
        pub const fn new() -> Self {
            Self {
                next: AtomicU64::new(1),
            }
        }
    }

    impl Subscriber for Null {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            false
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(self.next.fetch_add(1, Ordering::Relaxed))
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }
}

fn logger_init() -> anyhow::Result<flexi_logger::LoggerHandle> {
    use flexi_logger::{FileSpec, Logger, WriteMode};

    // rumqttd 用 tracing,装个空 subscriber 吞掉它的日志(走 set_global_default,不抢 log logger)。
    let _ = tracing::subscriber::set_global_default(null_subscriber::Null::new());

    // 日志统一收进 ~/.vibetty/logs/,不再散落在进程 CWD(herdr 插件 pane 会把 CWD 设成
    // 被分享 pane 的项目目录,以前每个项目都会被丢一个 log 文件)。文件名带上 cwd 的
    // basename(如 vibetty-vibekeys_firmware),区分不同项目;拿不到 cwd 兜底 default。
    let log_dir = dirs::home_dir()
        .map(|h| h.join(".vibetty").join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&log_dir).ok();
    let cwd_tag = std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "default".to_string());
    let file_spec = FileSpec::default()
        .suppress_timestamp()
        .directory(&log_dir)
        .basename(format!("vibetty-{cwd_tag}"));

    // 默认 info(RUST_LOG 优先)。rumqttc 用 log crate,强制 off 屏蔽。
    let spec = {
        let parsed = flexi_logger::LogSpecification::env_or_parse("info")?;
        let mut builder =
            flexi_logger::LogSpecBuilder::from_module_filters(parsed.module_filters());
        builder.module("rumqttc", flexi_logger::LevelFilter::Off);
        builder.finalize()
    };
    let logger = Logger::with(spec)
        .log_to_file(file_spec)
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
        Some(Commands::Skill { action }) => {
            if let Err(e) = skill::run_skill(action) {
                eprintln!("Skill error: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some(Commands::Herdr {
            target,
            quality,
            auto_submit,
        }) => {
            return run_herdr_mode(cli.config.clone(), target, quality, auto_submit).await;
        }
        Some(Commands::ShareHerdr) => {
            if let Err(e) = herdr_runner::run_share_herdr().await {
                log::error!("herdr share-herdr failed: {e}");
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

    let (cli_tx, cli_rx) = tokio::sync::mpsc::channel(1024);
    let (tx, rx) = tokio::sync::broadcast::channel(1024);
    drop(rx);

    let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(1024);

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

/// `vibetty herdr <target>` 模式:attach 到 herdr agent 终端,经 MQTT 分享。
/// `run_herdr` 自包含(自己 init TUI + 事件循环 + channel);这里只读配置后转发。
async fn run_herdr_mode(
    config: Option<std::path::PathBuf>,
    target: Option<String>,
    quality: String,
    auto_submit: bool,
) {
    // target:位置参数优先,否则读 VIBETTY_HERDR_TARGET env(herdr share action 开出的 pane 走 env)。
    let target = target
        .or_else(|| std::env::var(herdr_runner::HERDR_TARGET_ENV).ok())
        .filter(|s| !s.trim().is_empty());
    let Some(target) = target else {
        eprintln!(
            "Error: no target. Pass a target id or set {}.",
            herdr_runner::HERDR_TARGET_ENV
        );
        return;
    };
    log::info!("Starting Vibetty (herdr attach): target={target}");

    let image_format = config::parse_output_format(&quality);
    let mqtt_cfg = config::read_mqtt_config(config.as_deref());

    if let Err(e) = herdr_runner::run_herdr(target, mqtt_cfg, image_format, auto_submit).await {
        log::error!("Error in herdr attach execution: {}", e);
    }
}
