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

mod ui;

pub use vibetty_screenshot as screenshot;

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

    let (ui_tx, ui_rx) = tokio::sync::mpsc::channel(100);

    let asr_config = args.asr_config();
    log::info!("ASR Config: {:?}", asr_config);

    // 检查 VOSK 模型
    check_vosk_models(&asr_config);

    let (mut asr_interface, web_vosk_tx) = ws::ASRInterface::from_config(asr_config);

    let (screenshot_tx, screenshot_rx) = tokio::sync::mpsc::channel(4);

    let image_format = args.image_format();

    let state = ws::AppState {
        tx: tx.clone(),
        cli_tx,
        web_vosk_tx,
        screenshot_tx: screenshot_tx.clone(),
        image_format,
    };

    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .expect("Failed to bind to address");

    let listen_port = listener.local_addr().unwrap().port();

    // Spawn HTTP server
    let app = Router::new()
        .route("/", get(static_page::index_handler))
        .route("/app.js", get(static_page::app_js_handler))
        .route("/setup", get(static_page::setup_handler))
        .route("/vosk", get(static_page::vosk_handler))
        .route("/ws", get(ws::ws_handler))
        .route("/screenshot", get(ws::screenshot_handler))
        .route("/api/change-dir", post(static_page::change_dir_handler))
        .route("/vosk_ws", get(ws::web_vosk_ws_handler))
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
    let mut current_dir: Option<std::path::PathBuf> = None;
    let mut cli_rx = cli_rx;
    let mut ui_rx = ui_rx;
    let mut screenshot_rx = screenshot_rx;

    let command = args.command;
    loop {
        let r = ws::run_command(
            command.clone(),
            &mut asr_interface,
            cli_rx,
            &mut ui_rx,
            tx.clone(),
            current_dir,
            listen_port,
            screenshot_rx,
            &mut tui,
            &mut ui_title,
            &server_url,
            image_format,
        )
        .await;
        match r {
            Ok(ws::RunCommandResult::ChangeDir(new_path, returned_rx, returned_screenshot_rx)) => {
                log::info!("Changing directory to: {}", new_path);
                current_dir = Some(new_path.into());
                cli_rx = returned_rx;
                screenshot_rx = returned_screenshot_rx;
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

    ui::cleanup_terminal(&mut tui).ok();
}
