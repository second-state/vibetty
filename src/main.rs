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

use config::Args;

// 嵌入静态资源
const INDEX_HTML: &str = include_str!("../resources/index.html");
const APP_JS: &str = include_str!("../resources/app.js");

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

#[tokio::main]
async fn main() {
    dotenv::dotenv().ok();
    env_logger::init();

    let args = Args::parse();

    if args.command.is_empty() {
        eprintln!("Error: No command specified. Use -- to separate options and command.");
        std::process::exit(1);
    }

    log::info!("Starting Vibetty with command: {:?}", args);

    let (cli_tx, cli_rx) = tokio::sync::mpsc::channel(100);
    let (tx, rx) = tokio::sync::broadcast::channel(100);
    drop(rx);

    let state = ws::AppState {
        tx: tx.clone(),
        cli_tx,
    };

    let asr_config = args.asr_config();
    log::info!("ASR Config: {:?}", asr_config);

    tokio::spawn(async move {
        let command = args.command;
        let mut cli_rx = cli_rx;
        let mut current_dir: Option<std::path::PathBuf> = None;

        loop {
            let r = ws::run_command(
                command.clone(),
                asr_config.clone(),
                cli_rx,
                tx.clone(),
                current_dir,
            )
            .await;
            match r {
                Ok(ws::RunCommandResult::ChangeDir(new_path, returned_rx)) => {
                    log::info!("Changing directory to: {}", new_path);

                    current_dir = Some(new_path.into());
                    cli_rx = returned_rx;
                }
                Ok(ws::RunCommandResult::Done) => {
                    log::info!("Command execution finished");
                    std::process::exit(0);
                }
                Err(e) => {
                    log::error!("Error in command execution: {}", e);
                    std::process::exit(1);
                }
            }
        }
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(app_js_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/api/change-dir", post(change_dir_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .unwrap();

    log::info!("WebSocket server listening on ws://{}/ws", args.bind_addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
