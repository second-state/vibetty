use axum::{
    Router,
    body::Body,
    response::{IntoResponse, Response},
    routing::get,
};
use clap::Parser;

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
        let r = ws::run_command(args.command, asr_config, cli_rx, tx).await;
        if let Err(e) = r {
            log::error!("Error in command execution: {}", e);
            std::process::exit(1);
        } else {
            log::info!("Command execution finished");
            std::process::exit(0);
        }
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(app_js_handler))
        .route("/ws", get(ws::ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .unwrap();

    log::info!("WebSocket server listening on ws://{}/ws", args.bind_addr);

    axum::serve(listener, app).await.unwrap();
}
