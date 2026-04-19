use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use tokio::sync::{broadcast, mpsc};
use vt100::Callbacks;

use crate::{
    config::AsrConfig,
    protocol::{ClientMessage, ServerMessage, VoiceInputStart},
};

type ServerTx = broadcast::Sender<ServerMessage>;

type ClientTx = mpsc::Sender<ClientMessage>;
type ClientRx = mpsc::Receiver<ClientMessage>;

type OneshotSender<T> = tokio::sync::oneshot::Sender<T>;

type WebVoskReqTx = OneshotSender<(bytes::Bytes, OneshotSender<String>)>;
type WebVoskTx = mpsc::Sender<WebVoskReqTx>;
type WebVoskRx = mpsc::Receiver<WebVoskReqTx>;

struct WindowCallbacks {
    title: String,
    update_title: bool,
}

impl WindowCallbacks {
    fn new() -> Self {
        Self {
            title: String::new(),
            update_title: false,
        }
    }
}

impl Callbacks for WindowCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = std::str::from_utf8(title).unwrap().to_string();
        self.update_title = true;
    }
}

/// Screenshot request: send a oneshot sender to get the rendered JPEG bytes
type ScreenshotTx = mpsc::Sender<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>;

#[derive(Clone)]
pub struct AppState {
    pub tx: ServerTx,
    pub cli_tx: ClientTx,
    pub web_vosk_tx: Option<WebVoskTx>,
    pub screenshot_tx: ScreenshotTx,
}

fn t2s<S: AsRef<str>>(s: S) -> String {
    let s = hanconv::tw2sp(s.as_ref());
    s.replace("幺", "么")
}

pub enum ASRInterface {
    Whisper {
        client: reqwest::Client,
        config: crate::config::WhisperASRConfig,
    },
    WebVosk(WebVoskRx),
}

impl ASRInterface {
    pub fn from_config(config: AsrConfig) -> (Self, Option<WebVoskTx>) {
        match config {
            AsrConfig::Whisper(cfg) => (
                ASRInterface::Whisper {
                    client: reqwest::Client::new(),
                    config: cfg,
                },
                None,
            ),
            AsrConfig::WebVosk => {
                let (web_vosk_tx, web_vosk_rx) = mpsc::channel(10);
                (ASRInterface::WebVosk(web_vosk_rx), Some(web_vosk_tx))
            }
        }
    }

    pub async fn transcribe(&mut self, wav_data: Vec<u8>) -> anyhow::Result<String> {
        match self {
            ASRInterface::Whisper { client, config } => {
                let r = retry_whisper(
                    &client,
                    &config.url,
                    &config.api_key,
                    &config.model,
                    &config.lang,
                    &config.prompt,
                    wav_data,
                    3,
                    std::time::Duration::from_secs(5),
                )
                .await;

                Ok(t2s(r.join("\n")))
            }
            ASRInterface::WebVosk(rx) => {
                log::info!("Start recv a WebVosk transcription request");
                let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                let mut req = (bytes::Bytes::from(wav_data), resp_tx);

                loop {
                    let tx = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                        .await
                        .map_err(|_| anyhow::anyhow!("Request WebVosk timed out"))?
                        .ok_or(anyhow::anyhow!("WebVosk channel closed"))?;

                    if tx.is_closed() {
                        continue;
                    };

                    if let Err(e) = tx.send(req) {
                        log::warn!(
                            "Failed to send WebVosk transcription request, wait next request tx",
                        );
                        req = e;
                        continue;
                    } else {
                        break;
                    }
                }

                match tokio::time::timeout(std::time::Duration::from_secs(10), resp_rx).await {
                    Ok(Ok(transcription)) => Ok(transcription),
                    Ok(Err(_)) => Err(anyhow::anyhow!("WebVosk response channel closed")),
                    Err(_) => Err(anyhow::anyhow!("WebVosk transcription timed out")),
                }
            }
        }
    }
}


pub enum RunCommandResult {
    Done,
    ChangeDir(
        String,
        ClientRx,
        mpsc::Receiver<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    ),
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    command: Vec<String>,
    asr_interface: &mut ASRInterface,
    mut rx: ClientRx,
    ui_rx: &mut mpsc::Receiver<crate::ui::UIEvent>,
    tx: ServerTx,
    current_dir: Option<std::path::PathBuf>,
    listen_port: u16,
    mut screenshot_rx: mpsc::Receiver<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    tui: &mut crate::ui::TuiTerminal,
    ui_title: &mut String,
    ui_footer: &str,
) -> anyhow::Result<RunCommandResult> {
    let dir_path = current_dir
        .as_ref()
        .and_then(|p| p.to_str())
        .unwrap_or(".")
        .to_string();
    let _ = tx.send(ServerMessage::notification(
        crate::protocol::NotificationLevel::Info,
        format!("Working in: {}", dir_path),
    ));

    enum TerminalEvent {
        Input(crate::protocol::ClientMessage),
        InputClosed,

        UIEvent(crate::ui::UIEvent),

        PtyOutput(String),

        Error,
    }

    let mut terminal = crate::terminal::pty::new_with_command(
        command.first().unwrap().as_str(),
        &command[1..],
        &[("VIBETTY_PORT".to_string(), listen_port.to_string())],
        (24, 80),
        current_dir,
    )
    .await?;

    let mut wav_buffer = Vec::new();
    let mut wav_sample_rate = 16000;

    let mut vt_parser = vt100::Parser::new_with_callbacks(24, 80, 8096, WindowCallbacks::new());

    loop {
        let terminal_read_event = terminal.read_pty_output();

        let event = tokio::select! {
            result = terminal_read_event => {
                match result {
                    Ok(r) => TerminalEvent::PtyOutput(r),
                    Err(e) => {
                        log::error!("[{}] Error reading PTY output: {:?}", terminal.session_id(), e);
                        TerminalEvent::Error
                    },
                }

            },
            msg = rx.recv() => {
                match msg {
                    Some(input) => TerminalEvent::Input(input),
                    None => TerminalEvent::InputClosed,
                }
            },

            ui_evt = ui_rx.recv() => {
                match ui_evt {
                    Some(evt) => TerminalEvent::UIEvent(evt),
                    None => {
                        log::error!("[{}] UI event channel closed", terminal.session_id());
                        TerminalEvent::Error
                    }
                }
            },

            req = screenshot_rx.recv() => {
                if let Some(resp_tx) = req {
                    let screen = vt_parser.screen().clone();
                    let result = tokio::task::spawn_blocking(move || {
                        render_screen_to_jpeg(&screen)
                    }).await;
                    let jpeg = match result {
                        Ok(Ok(data)) => Ok(data),
                        Ok(Err(e)) => Err(e),
                        Err(e) => Err(e.to_string()),
                    };
                    let _ = resp_tx.send(jpeg);
                }
                continue;
            }
        };

        match event {
            TerminalEvent::PtyOutput(output) => {
                log::trace!("[{}] PTY output: {}", terminal.session_id(), output.len());
                vt_parser.process(output.as_bytes());

                // Check for title update from callbacks
                {
                    let cb = vt_parser.callbacks_mut();
                    if cb.update_title {
                        cb.update_title = false;
                        let new_title = cb.title.clone();
                        let _ = cb;
                        *ui_title = new_title.clone();

                        // Send title change to client
                        let _ = tx.send(ServerMessage::title(
                            new_title,
                        ));

                        vt_parser.screen_mut().set_scrollback(0);
                    }
                }

                // Render directly to TUI
                let screen = vt_parser.screen().clone();
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ = tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }
            }

            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(input)) => {
                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollUp) => {
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s + 5);
                let screen = vt_parser.screen().clone();
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ = tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollDown) => {
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s.saturating_sub(5));
                let screen = vt_parser.screen().clone();
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ = tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Resize(cols, rows)) => {
                vt_parser.screen_mut().set_size(rows.saturating_sub(6) as u16, cols.saturating_sub(4) as u16);
            }
            TerminalEvent::Input(ClientMessage::Sync) => {
                log::info!("Received Sync message from client");
            }
            TerminalEvent::Input(ClientMessage::PtyInput(input)) => {
                log::info!(
                    "Sending input to terminal: {:?}",
                    String::from_utf8_lossy(&input)
                );

                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::Input(ClientMessage::Input(text)) => {
                log::info!("Sending text input to terminal: {:?}", text);
                terminal.send_text(&text).await?;
            }
            TerminalEvent::Input(ClientMessage::ChangeDir(path)) => {
                log::info!("Change directory requested: {}", path);
                let _ = tx.send(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Info,
                    format!("Changing directory to: {}", path),
                ));
                return Ok(RunCommandResult::ChangeDir(path, rx, screenshot_rx));
            }
            TerminalEvent::Input(ClientMessage::VoiceInputStart(VoiceInputStart {
                sample_rate,
            })) => {
                log::info!("Voice input started with sample rate: {:?}", sample_rate);
                wav_buffer.clear();
                wav_sample_rate = sample_rate.unwrap_or(16000);
            }
            TerminalEvent::Input(ClientMessage::VoiceInputChunk(chunk)) => {
                wav_buffer.extend_from_slice(&chunk);
            }
            TerminalEvent::Input(ClientMessage::VoiceInputEnd(..)) => {
                log::info!("Voice input ended, total size: {} bytes", wav_buffer.len());
                let config = crate::util::WavConfig {
                    sample_rate: wav_sample_rate,
                    channels: 1,
                    bits_per_sample: 16,
                };
                let wav_data = crate::util::pcm_to_wav(&wav_buffer, config);
                if std::env::var("ASR_DEBUG_WAV").is_ok() {
                    let debug_path = format!("debug_{}.wav", terminal.session_id());
                    if let Err(e) = std::fs::write(&debug_path, &wav_data) {
                        log::error!("Failed to write debug WAV file: {}", e);
                    } else {
                        log::info!("Saved debug WAV file: {}", debug_path);
                    }
                }

                let mut asr_text = match asr_interface.transcribe(wav_data).await {
                    Ok(mut text) => {
                        log::info!("ASR transcription result: {}", text);
                        text.push(' ');
                        text
                    }
                    Err(e) => {
                        log::error!("ASR transcription failed: {}", e);
                        format!("ASR Error: {}", e)
                    }
                };

                // 如果 ASR 结果等于环境变量 VIBETTY_EXIT_COMMAND 的值，替换为 "/exit"（大小写不敏感）
                if let Ok(exit_trigger) = std::env::var("VIBETTY_EXIT_COMMAND")
                    && asr_text.trim().to_lowercase() == exit_trigger.trim().to_lowercase()
                {
                    asr_text = "/exit".to_string();
                }

                if let Err(_e) = tx.send(ServerMessage::asr_result(asr_text)) {
                    log::error!("[{}] No client waiting for data", terminal.session_id());
                }
            }
            TerminalEvent::InputClosed | TerminalEvent::Error => {
                log::error!("Input channel closed or error occurred, terminating terminal loop");
                break;
            }
        }
    }

    Ok(RunCommandResult::Done)
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut server_rx = state.tx.subscribe();
    if state.cli_tx.send(ClientMessage::Sync).await.is_err() {
        log::error!("Failed to send Sync message to cli_tx");
        return;
    }

    let mut wait_pong = false;
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if wait_pong {
                    log::error!("No pong received, closing WebSocket connection");
                    break;
                }
                wait_pong = true;
                if socket.send(Message::Ping(bytes::Bytes::new())).await.is_err() {
                    log::error!("Failed to send ping message");
                    break;
                }
            }
            // 接收来自服务器的消息（广播），发送到 WebSocket 客户端
            result = server_rx.recv() => {
                match result {
                    Ok(msg) => {
                        let data = msg.to_msgpack().unwrap();
                        if socket.send(Message::Binary(data.into())).await.is_err() {
                            log::error!("Failed to send message to WebSocket");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("Lagged behind by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        log::error!("Broadcast channel closed");
                        break;
                    }
                }
            }
            // 接收来自 WebSocket 客户端的消息
            result = socket.recv() => {
                match result {
                    Some(Ok(msg)) => match msg {
                        Message::Binary(data) => {
                            log::trace!("Received binary message, length: {}", data.len());
                            match ClientMessage::from_msgpack(&data) {
                                Ok(client_msg) => {
                                    log::debug!("Parsed client message: {:?}", client_msg);
                                    if let Err(e) = state.cli_tx.send(client_msg).await {
                                        log::error!("Failed to send client message: {}", e);
                                        break;
                                    }
                                    log::debug!("Successfully sent client message to cli_tx");
                                }
                                Err(e) => {
                                    log::error!("Failed to parse message: {}", e);
                                    log::error!("MessagePack data (hex): {:02x?}", &data[..data.len().min(32)]);
                                }
                            }
                        }
                        Message::Text(text) => {
                            match ClientMessage::from_json(&text) {
                                Ok(client_msg) => {
                                    log::info!("Received: {:?}", client_msg);
                                    if let Err(e) = state.cli_tx.send(client_msg).await {
                                        log::error!("Failed to send client message: {}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to parse JSON message: {}", e);
                                }
                            }
                        }
                        Message::Close(_) => {
                            log::info!("Client disconnected");
                            break;
                        }
                        Message::Pong(_) => {
                            wait_pong = false;
                            log::debug!("Received pong from client");
                        }
                        _ => {}
                    },
                    Some(Err(e)) => {
                        log::error!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        log::info!("WebSocket connection closed");
                        break;
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn retry_whisper(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    model: &str,
    lang: &str,
    prompt: &str,
    wav_audio: Vec<u8>,
    retry: usize,
    timeout: std::time::Duration,
) -> Vec<String> {
    for i in 0..retry {
        log::debug!("Attempting ASR request, try {}/{}", i + 1, retry);
        let r = tokio::time::timeout(
            timeout,
            crate::asr::whisper(client, url, api_key, model, lang, prompt, wav_audio.clone()),
        )
        .await;
        match r {
            Ok(Ok(v)) => {
                log::info!("ASR successful on try {}/{}, result: {:?}", i + 1, retry, v);
                return v;
            }
            Ok(Err(e)) => {
                log::error!("asr error: {e}");
                continue;
            }
            Err(_) => {
                log::error!("asr timeout, retry {i}");
                continue;
            }
        }
    }
    vec![]
}

pub async fn web_vosk_ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    if let Some(tx) = state.web_vosk_tx {
        ws.on_upgrade(move |socket| handle_web_vosk_socket(socket, tx))
    } else {
        log::error!("WebVosk ASR interface not configured");
        // 直接返回一个错误响应
        axum::response::Response::builder()
            .status(400)
            .body("WebVosk ASR interface not configured".into())
            .unwrap()
    }
}

async fn handle_web_vosk_socket(mut socket: WebSocket, vosk_tx: WebVoskTx) {
    loop {
        let (tx, rx) = tokio::sync::oneshot::channel();
        if vosk_tx.send(tx).await.is_err() {
            log::error!("Failed to send WebVosk request channel");
            break;
        }

        match rx.await {
            Ok((wav_data, resp_tx)) => {
                log::info!(
                    "Received WebVosk transcription request, data length: {}",
                    wav_data.len()
                );
                socket
                    .send(Message::Binary(wav_data))
                    .await
                    .unwrap_or_else(|e| {
                        log::error!("Failed to send WAV data to WebVosk client: {}", e);
                    });

                match socket.recv().await {
                    Some(Ok(Message::Binary(_))) => {
                        log::warn!(
                            "Unexpected binary message from WebVosk client, expected transcription result"
                        );
                    }
                    Some(Ok(Message::Text(text))) => {
                        log::info!(
                            "Received transcription result from WebVosk client: {}",
                            text
                        );
                        if resp_tx.send(text.to_string()).is_err() {
                            log::error!("Failed to send transcription response back to requester");
                            continue;
                        }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        log::trace!("Received ping from WebVosk client");
                    }
                    Some(Ok(Message::Pong(_))) => {
                        log::trace!("Received pong from WebVosk client");
                    }
                    Some(Ok(Message::Close(_))) => {
                        log::info!("WebVosk client disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        log::error!("WebSocket error while waiting for  result: {}", e);
                        break;
                    }
                    None => {
                        log::error!("WebSocket connection closed");
                        break;
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to receive WebVosk request: {}", e);
                continue;
            }
        }
    }
}

/// Render a vt100 screen to JPEG bytes
fn render_screen_to_jpeg(screen: &vt100::Screen) -> Result<Vec<u8>, String> {
    let config = crate::screenshot::ScreenshotConfig::default();
    let image = crate::screenshot::capture_screen(screen, &config)
        .map_err(|e| format!("Failed to capture screen: {}", e))?;

    // Convert RGBA to RGB for JPEG
    let rgb_image = image::DynamicImage::ImageRgba8(image).to_rgb8();
    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 85);
    encoder
        .encode(
            rgb_image.as_raw(),
            rgb_image.width(),
            rgb_image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| format!("Failed to encode JPEG: {}", e))?;

    Ok(jpeg_buf.into_inner())
}

/// HTTP handler for /screenshot.jpeg
pub async fn screenshot_handler(State(state): State<AppState>) -> impl IntoResponse {
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

    if state.screenshot_tx.send(resp_tx).await.is_err() {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            "Screenshot service unavailable".to_string(),
        )
            .into_response();
    }

    match resp_rx.await {
        Ok(Ok(jpeg_data)) => {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                "image/jpeg".parse().unwrap(),
            );
            headers.insert(
                axum::http::header::CACHE_CONTROL,
                "no-cache".parse().unwrap(),
            );
            (axum::http::StatusCode::OK, headers, jpeg_data).into_response()
        }
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            format!("Failed to render screenshot: {}", e),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            "Screenshot request timed out".to_string(),
        )
            .into_response(),
    }
}
