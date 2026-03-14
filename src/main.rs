use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::Parser;
use serde::Deserialize;
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

// 嵌入静态资源
const INDEX_HTML: &str = include_str!("../resources/index.html");
const APP_JS: &str = include_str!("../resources/app.js");
const SETUP_HTML: &str = include_str!("../resources/setup.html");

async fn index_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(INDEX_HTML))
        .unwrap()
}

async fn app_js_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "application/javascript")
        .body(Body::from(APP_JS))
        .unwrap()
}

async fn setup_handler() -> impl IntoResponse {
    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(SETUP_HTML))
        .unwrap()
}

#[derive(Deserialize)]
struct ChangeDirRequest {
    path: String,
}

async fn change_dir_handler(
    State(state): State<ws::AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<ChangeDirRequest>,
) -> impl IntoResponse {
    // 检查是否来自 localhost
    let ip = addr.ip();
    let is_localhost = ip.is_loopback();

    if !is_localhost {
        log::warn!("Change directory request from non-localhost: {}", ip);
        return (StatusCode::FORBIDDEN, "Only localhost access allowed").into_response();
    }

    log::info!("Change directory request from {}: {}", ip, req.path);

    // 发送 ChangeDir 消息到 cli_tx
    if let Err(e) = state
        .cli_tx
        .send(crate::protocol::ClientMessage::ChangeDir(req.path.clone()))
        .await
    {
        log::error!("Failed to send ChangeDir message: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send change directory command: {}", e),
        )
            .into_response();
    }

    (StatusCode::OK, format!("Changing to: {}", req.path)).into_response()
}

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

    let state = ws::AppState {
        tx: tx.clone(),
        cli_tx,
    };

    let asr_config = args.asr_config();
    log::info!("ASR Config: {:?}", asr_config);

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
                asr_config.clone(),
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
        .route("/", get(index_handler))
        .route("/app.js", get(app_js_handler))
        .route("/setup", get(setup_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/api/change-dir", post(change_dir_handler))
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
