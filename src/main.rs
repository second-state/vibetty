use axum::{
    Router,
    routing::{get, post},
};
use clap::Parser;
use std::net::SocketAddr;

mod asr;
mod config;
mod protocol;
mod util;
mod ws;

mod terminal;
mod types;

mod ui;

use config::Args;

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

    let args = Args::parse();

    if args.command.is_empty() {
        eprintln!("Error: No command specified. Use -- to separate options and command.");
        std::process::exit(1);
    }

    log::info!("Starting Vibetty with command: {:?}", args);

    let (cli_tx, cli_rx) = tokio::sync::mpsc::channel(100);
    let (tx, rx) = tokio::sync::broadcast::channel(100);
    drop(rx);

    let (pty_output_tx, pty_output_rx) = tokio::sync::mpsc::channel(100);
    let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(100);

    let asr_config = args.asr_config();
    log::info!("ASR Config: {:?}", asr_config);

    let (mut asr_interface, web_vosk_tx) = ws::ASRInterface::from_config(asr_config);

    let state = ws::AppState {
        tx: tx.clone(),
        cli_tx,
        web_vosk_tx,
    };

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .expect("Failed to bind to address");

    let listen_port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let command = args.command;
        let mut cli_rx = cli_rx;
        let mut current_dir: Option<std::path::PathBuf> = None;
        let mut ui_rx = ui_rx;

        loop {
            let r = ws::run_command(
                command.clone(),
                &mut asr_interface,
                cli_rx,
                ui_rx,
                tx.clone(),
                pty_output_tx.clone(),
                current_dir,
                listen_port,
            )
            .await;
            match r {
                Ok(ws::RunCommandResult::ChangeDir(new_path, returned_rx, returned_ui_rx)) => {
                    log::info!("Changing directory to: {}", new_path);

                    current_dir = Some(new_path.into());
                    cli_rx = returned_rx;
                    ui_rx = returned_ui_rx;
                }
                Ok(ws::RunCommandResult::Done) => {
                    log::info!("Command execution finished");
                    break;
                }
                Err(e) => {
                    log::error!("Error in command execution: {}", e);
                    break;
                }
            }
        }
    });

    let server_url = if let Ok(addr) = listener.local_addr() {
        let port = addr.port();
        let addr_ip = addr.ip();
        if addr_ip.is_loopback() {
            Some(format!(
                "http://localhost:{}        Warning: Server only bind on loopback dev. ",
                port
            ))
        } else {
            Some(format!("http://{}:{}\n", addr.ip(), port))
        }
    } else {
        None
    }
    .unwrap_or("Warning: Failed to get a valid server URL\n".to_string());

    let mut ui_app = ui::App::new("Vibetty".to_string(), "Footer".to_string(), pty_output_rx);
    let r = tokio::task::spawn_blocking(move || {
        if let Err(e) = ui_app.run(ui_tx, server_url) {
            log::error!("UI error: {}", e);
        }
    });

    let app = Router::new()
        .route("/", get(static_page::index_handler))
        .route("/app.js", get(static_page::app_js_handler))
        .route("/setup", get(static_page::setup_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/api/change-dir", post(static_page::change_dir_handler))
        .route("/vosk_ws", get(ws::web_vosk_ws_handler))
        .with_state(state);

    log::info!("WebSocket server listening on ws://{}/ws", args.bind_addr);
    log::info!("HTTP server listening on http://{}", args.bind_addr);

    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );

    tokio::select! {
        res = serve => {
            if let Err(e) = res {
                log::error!("Server error: {}", e);
            }
        }
        _ = r => {
            log::info!("UI thread finished");
        }
    }
}
