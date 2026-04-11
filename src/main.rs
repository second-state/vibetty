use axum::{
    Router,
    routing::{get, post},
};
use clap::Parser;
use std::env;
use std::net::SocketAddr;
use tower_http::services::ServeDir;

mod asr;
mod config;
mod protocol;
mod util;
mod ws;

mod terminal;
mod types;

mod ui;

mod screenshot;

use config::{Args, AsrConfig};

mod static_page;

fn check_vosk_models(asr_config: &AsrConfig) {
    if !matches!(asr_config, AsrConfig::WebVosk) {
        return;
    }

    let models_dir = match env::home_dir() {
        Some(home) => home.join(".vibetty/models"),
        None => {
            log::warn!("Failed to get home directory");
            return;
        }
    };

    // 检查并创建目录
    if !models_dir.exists() {
        if let Err(e) = std::fs::create_dir_all(&models_dir) {
            log::warn!(
                "Failed to create models directory {}: {}",
                models_dir.display(),
                e
            );
            return;
        }
        log::info!("Created models directory: {}", models_dir.display());
    }

    // 检查模型文件
    let models = [
        ("vosk-model-small-cn-0.22.zip", "Chinese model"),
        ("vosk-model-small-en-us-0.15.zip", "English model"),
    ];

    let mut missing = Vec::new();
    for (filename, _desc) in models {
        let path = models_dir.join(filename);
        if !path.exists() {
            missing.push((filename, _desc));
        }
    }

    if !missing.is_empty() {
        println!("==========================================");
        println!("VOSK model files missing:");
        for (filename, desc) in &missing {
            println!("  - {} ({})", filename, desc);
        }
        println!();
        println!("Download models to: {}", models_dir.display());
        println!();
        println!("Download commands:");
        println!("  # Chinese model");
        println!(
            "  wget -P {} https://alphacephei.com/vosk/models/vosk-model-small-cn-0.22.zip",
            models_dir.display()
        );
        println!(
            "  # or: curl -o {} https://alphacephei.com/vosk/models/vosk-model-small-cn-0.22.zip",
            models_dir.join("vosk-model-small-cn-0.22.zip").display()
        );
        println!();
        println!("  # English model");
        println!(
            "  wget -P {} https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip",
            models_dir.display()
        );
        println!(
            "  # or: curl -o {} https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip",
            models_dir.join("vosk-model-small-en-us-0.15.zip").display()
        );
        println!();
        println!("After download, models will be available at /models/ path");
        println!("==========================================");
        std::process::exit(1);
    } else {
        log::info!(
            "VOSK models check passed: all models found in {}",
            models_dir.display()
        );
    }
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

    let asr_config = args.asr_config();
    log::info!("ASR Config: {:?}", asr_config);

    // 检查 VOSK 模型
    check_vosk_models(&asr_config);

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
        .route("/vosk", get(static_page::vosk_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/api/change-dir", post(static_page::change_dir_handler))
        .route("/vosk_ws", get(ws::web_vosk_ws_handler))
        .nest_service(
            "/models",
            ServeDir::new(env::home_dir().unwrap().join(".vibetty/models")),
        )
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
