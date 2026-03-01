use axum::{
    Router,
    routing::{get, get_service},
};
use clap::Parser;

mod asr;
mod config;
mod protocol;
mod util;
mod ws;

use config::Args;

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
        } else {
            log::info!("Command execution finished");
        }
    });

    let app = Router::new()
        .route("/ws", get(ws::ws_handler))
        .fallback_service(get_service(tower_http::services::ServeDir::new(
            "resources",
        )))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .unwrap();

    log::info!("WebSocket server listening on ws://{}/ws", args.bind_addr);

    axum::serve(listener, app).await.unwrap();
}
