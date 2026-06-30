use axum::{Router, routing::get};
use clap::Parser;
use std::env;
use std::net::SocketAddr;
use tower_http::services::ServeDir;

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
        .log_to_file(FileSpec::default())
        .write_mode(WriteMode::BufferAndFlush)
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
            if let Err(e) = setup::run_setup() {
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

    let state = ws::AppState {
        tx: tx.clone(),
        cli_tx,
        screenshot_tx: screenshot_tx.clone(),
        image_format,
    };

    // 可选 MQTT 传输:配置里有 [mqtt] 段且 enable!=false 才启用,否则完全不碰。
    match args.mqtt_config() {
        Some(cfg) if cfg.enable => {
            mqtt::spawn(cfg, state.cli_tx.clone(), state.tx.clone(), image_format);
            log::info!("[mqtt] transport enabled");
        }
        Some(_) => log::info!("[mqtt] transport disabled by config (enable=false)"),
        None => log::debug!("[mqtt] not configured, transport disabled (WebSocket/HTTP only)"),
    }

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .expect("Failed to bind to address");

    let listen_port = listener.local_addr().unwrap().port();

    // Spawn HTTP server
    let app = Router::new()
        .route("/", get(static_page::index_handler))
        .route("/app.js", get(static_page::app_js_handler))
        .route("/vosk", get(static_page::vosk_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/screenshot", get(ws::screenshot_handler))
        .nest_service(
            "/models",
            ServeDir::new(env::home_dir().unwrap().join(".vibetty/models")),
        )
        .with_state(state);

    log::info!("WebSocket server listening on ws://{}/ws", args.bind_addr);
    log::info!("HTTP server listening on http://{}", args.bind_addr);

    tokio::spawn(async move {
        let serve = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        );
        if let Err(e) = serve.await {
            log::error!("Server error: {}", e);
        }
    });

    // Init TUI
    let mut tui = ui::init_terminal().expect("Failed to initialize terminal");
    ui::spawn_event_loop(ui_tx);

    let server_url = if let Ok(addr) = std::net::TcpListener::bind(&args.bind_addr) {
        let addr = addr.local_addr().unwrap();
        if addr.ip().is_loopback() {
            format!(
                "http://localhost:{}        Warning: Server only bind on loopback dev. ",
                listen_port
            )
        } else {
            format!("http://{}:{}", addr.ip(), listen_port)
        }
    } else {
        format!("http://localhost:{}", listen_port)
    };

    let mut ui_title = String::new();
    let mut ui_rx = ui_rx;

    let command = args.command;
    if let Err(e) = ws::run_command(
        command,
        cli_rx,
        &mut ui_rx,
        tx.clone(),
        listen_port,
        screenshot_rx,
        &mut tui,
        &mut ui_title,
        &server_url,
        image_format,
        args.auto_submit,
    )
    .await
    {
        log::error!("Error in command execution: {}", e);
    }

    ui::cleanup_terminal(&mut tui).ok();
}
