use std::sync::Arc;

use axum::{
    extract::{
        Query, State,
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

/// Image broadcast frame interval (ms)
const IMAGE_FRAME_INTERVAL_MS: u64 = 500;
/// Image chunk size (bytes)
const IMAGE_CHUNK_SIZE: usize = 10 * 1024;
/// Columns reserved for TUI decorations (borders)
const TUI_COLS_PADDING: u16 = 4;
/// Rows reserved for TUI decorations (title + footer)
const TUI_ROWS_PADDING: u16 = 6;

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

fn send_screen(tx: &ServerTx, screen: Arc<vt100::Screen>) {
    // if let Ok(jpeg) = render_screen_to_jpeg(&screen) {
    //     let chunk_size = IMAGE_CHUNK_SIZE;
    //     let total_chunks = (jpeg.len() + chunk_size - 1) / chunk_size;
    //     for (i, chunk) in jpeg.chunks(chunk_size).enumerate() {
    //         let is_last = i == total_chunks - 1;
    //         let _ = tx.send(ServerMessage::screen_image_chunk(
    //             crate::protocol::ImageFormat::Jpeg,
    //             is_last,
    //             chunk.to_vec(),
    //         ));
    //     }
    // }

    let _ = tx.send(ServerMessage::Screen(screen));
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

        ScreenGetter(tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>),

        Error,
    }

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let vt_cols = cols - TUI_COLS_PADDING;
    let vt_rows = rows - TUI_ROWS_PADDING;

    let mut terminal = crate::terminal::pty::new_with_command(
        command.first().unwrap().as_str(),
        &command[1..],
        &[("VIBETTY_PORT".to_string(), listen_port.to_string())],
        (vt_rows, vt_cols),
        current_dir,
    )
    .await?;

    let mut wav_buffer = Vec::new();
    let mut wav_sample_rate = 16000;

    let mut vt_parser =
        vt100::Parser::new_with_callbacks(vt_rows, vt_cols, 8096, WindowCallbacks::new());

    // Frame rate limit for image broadcast (default 2 fps)
    let frame_interval = std::time::Duration::from_millis(IMAGE_FRAME_INTERVAL_MS);
    let mut last_frame_time = std::time::Instant::now() - frame_interval;

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
                match req {
                    Some(resp_tx) => TerminalEvent::ScreenGetter(resp_tx),
                    None => {
                        log::error!("[{}] Screenshot request channel closed", terminal.session_id());
                        TerminalEvent::Error
                    }
                }
            }
        };

        match event {
            TerminalEvent::ScreenGetter(getter) => {
                let screen = vt_parser.screen().clone();
                let mut window_scrollback = 0;
                let result = render_screen_to_jpeg(&screen, None, &mut window_scrollback);

                let jpeg = match result {
                    Ok(data) => Ok(data),
                    Err(e) => Err(e.to_string()),
                };
                let _ = getter.send(jpeg);
            }
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
                        let _ = tx.send(ServerMessage::title(new_title));
                    }
                }

                // Render directly to TUI
                let screen = Arc::new(vt_parser.screen().clone());
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }

                // Generate JPEG and broadcast chunks for img subscribers (rate limited)
                let now = std::time::Instant::now();
                if now.duration_since(last_frame_time) >= frame_interval {
                    last_frame_time = now;
                    send_screen(&tx, screen);
                }
            }

            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(input)) => {
                log::info!("UI Input: {:?}", String::from_utf8_lossy(&input));
                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollUp)
            | TerminalEvent::Input(ClientMessage::ScrollUp) => {
                log::info!("ScrollUp");
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s + 5);
                let screen = Arc::new(vt_parser.screen().clone());
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));

                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollDown)
            | TerminalEvent::Input(ClientMessage::ScrollDown) => {
                log::info!("ScrollDown");
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s.saturating_sub(5));
                let screen = Arc::new(vt_parser.screen().clone());
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));

                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Resize(cols, rows)) => {
                log::info!("Resize: cols={}, rows={}", cols, rows);
                let vt_cols = cols.saturating_sub(TUI_COLS_PADDING);
                let vt_rows = rows.saturating_sub(TUI_ROWS_PADDING);
                vt_parser.screen_mut().set_size(vt_rows, vt_cols);
                let _ = terminal.resize(vt_rows, vt_cols);
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
            }
            TerminalEvent::Input(ClientMessage::Sync) => {
                log::info!("Received Sync message from client, sending screen");
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
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

#[derive(Debug, Clone, serde::Deserialize)]
pub struct WsParams {
    #[serde(default = "default_true")]
    pub pty: bool,
    #[serde(default)]
    pub img: bool,
    /// Client screen width (columns)
    #[serde(default = "default_width")]
    pub width: u16,
    /// Client screen height (rows)
    #[serde(default = "default_height")]
    pub height: u16,
}

fn default_true() -> bool {
    true
}

fn default_height() -> u16 {
    240
}

fn default_width() -> u16 {
    240
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    log::info!(
        "WebSocket connection params: pty={}, img={}",
        params.pty,
        params.img
    );
    ws.on_upgrade(move |socket| handle_socket(socket, state, params))
}

async fn send_screen_to_client(
    state: &AppState,
    socket: &mut WebSocket,
    screen: &vt100::Screen,
    window_size: Option<(u16, u16)>, // (width, height)
    window_h_offset: &mut u16,
) -> anyhow::Result<()> {
    let jpeg = render_screen_to_jpeg(&screen, window_size, window_h_offset)?;
    if jpeg.is_empty() {
        state
            .cli_tx
            .send(ClientMessage::ScrollDown)
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to send ScrollDown message to cli_tx after empty JPEG: {}",
                    e
                )
            })?;
    }
    log::debug!(
        "Sending screen JPEG to client, size: {} KB",
        jpeg.len() / 1024
    );
    let chunk_size = IMAGE_CHUNK_SIZE;
    let total_chunks = (jpeg.len() + chunk_size - 1) / chunk_size;
    for (i, chunk) in jpeg.chunks(chunk_size).enumerate() {
        let is_last = i == total_chunks - 1;
        let msg = ServerMessage::screen_image_chunk(
            crate::protocol::ImageFormat::Jpeg,
            is_last,
            chunk.to_vec(),
        );
        let data = msg.to_msgpack()?;
        socket.send(Message::Binary(data.into())).await?;
    }
    Ok(())
}

async fn handle_client_message(
    state: &AppState,
    msg: ClientMessage,
    height: u16,
    window_h_offset: &mut u16,
) -> anyhow::Result<()> {
    log::debug!("Handling client message: {:?}", msg);
    match msg {
        ClientMessage::ScrollUp => {
            if *window_h_offset == 0 {
                state
                    .cli_tx
                    .send(ClientMessage::ScrollUp)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to send client message: {}", e))?;
                return Ok(());
            }
            if *window_h_offset > height / 2 {
                *window_h_offset -= height / 2
            } else {
                *window_h_offset = 0
            };

            log::debug!("ScrollUp, new window_h_offset: {}", window_h_offset);
            state
                .cli_tx
                .send(ClientMessage::Sync)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send client message: {}", e))?;
        }
        ClientMessage::ScrollDown => {
            *window_h_offset += height / 2;

            log::debug!("ScrollDown, new window_h_offset: {}", window_h_offset);
            state
                .cli_tx
                .send(ClientMessage::Sync)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send client message: {}", e))?;
        }
        msg => {
            state
                .cli_tx
                .send(msg)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to send client message: {}", e))?;
        }
    }
    Ok(())
}

async fn handle_socket(mut socket: WebSocket, state: AppState, params: WsParams) {
    let mut server_rx = state.tx.subscribe();
    if state.cli_tx.send(ClientMessage::Sync).await.is_err() {
        log::error!("Failed to send Sync message to cli_tx");
        return;
    }

    let mut wait_pong = false;
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));

    let window_size = (params.width, params.height);
    let mut window_h_offset = 0;

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
                        // Filter based on subscription params
                        match &msg {
                            ServerMessage::PtyOutput(_) if !params.pty => continue,
                            ServerMessage::ScreenImage(_) => {
                                log::warn!("unexpected ScreenImage message");
                                continue
                            },
                            ServerMessage::Screen(screen) => {
                                if let Err(e) = send_screen_to_client(&state, &mut socket, screen, Some(window_size), &mut window_h_offset).await {
                                    log::error!("Failed to send screen to client: {}", e);
                                }
                                continue;
                            },
                            _ => {}
                        }
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
                                    if let Err(e) = handle_client_message(&state, client_msg, window_size.1, &mut window_h_offset).await {
                                        log::error!("Failed to handle client message: {}", e);
                                        break;
                                    }
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
                                    if let Err(e) = handle_client_message(&state, client_msg, window_size.1, &mut window_h_offset).await {
                                        log::error!("Failed to handle client message: {}", e);
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
fn render_screen_to_jpeg(
    screen: &vt100::Screen,
    window_size: Option<(u16, u16)>, // (width, height)
    window_h_offset: &mut u16,
) -> anyhow::Result<Vec<u8>> {
    let config = crate::screenshot::ScreenshotConfig {
        show_decorations: false,
        ..Default::default()
    };

    let image = crate::screenshot::capture_screen(screen, &config)
        .map_err(|e| anyhow::anyhow!("Failed to capture screen: {}", e))?;

    // Convert RGBA to RGB for JPEG
    let mut rgb_image = image::DynamicImage::ImageRgba8(image).to_rgb8();

    if let Some((width, height)) = window_size {
        let orig_width = rgb_image.width();
        let orig_height = rgb_image.height();
        log::debug!(
            "Original image size: {}x{}, requested window size: {}x{}",
            orig_width,
            orig_height,
            width,
            height
        );
        let scale = width as f32 / orig_width as f32;
        let new_height = (orig_height as f32 * scale).round() as u32;

        rgb_image = image::imageops::resize(
            &rgb_image,
            width as u32,
            new_height,
            image::imageops::FilterType::Lanczos3,
        );

        // 根据垂直偏移截取
        let y_offset = *window_h_offset as u32;
        let crop_height = height as u32;

        if y_offset + crop_height > new_height {
            if crop_height > new_height {
                log::warn!(
                    "Requested crop height {} exceeds image height {}, adjusting to fit",
                    crop_height,
                    new_height
                );
                *window_h_offset = 0;
            } else {
                log::warn!(
                    "Vertical offset {} + crop height {} exceeds image height {}, adjusting offset",
                    y_offset,
                    crop_height,
                    new_height
                );
                *window_h_offset = (new_height - crop_height) as u16;
            }
            return Ok(Vec::new());
        }

        rgb_image = image::imageops::crop(&mut rgb_image, 0, y_offset, width as u32, crop_height)
            .to_image();
    }

    let mut jpeg_buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 100);
    encoder
        .encode(
            rgb_image.as_raw(),
            rgb_image.width(),
            rgb_image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| anyhow::anyhow!("Failed to encode JPEG: {}", e))?;

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
