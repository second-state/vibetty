use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

use crate::{
    config::AsrConfig,
    protocol::{ChoicesData, ClientMessage, ServerMessage, VoiceInputStart},
    terminal::claude::{ClaudeCodeResult, ClaudeCodeState, UseTool},
};

/// AskUserQuestion 工具输入结构
#[derive(Debug, Clone, Deserialize)]
struct AskUserQuestionInput {
    questions: Vec<Question>,
}

#[derive(Debug, Clone, Deserialize)]
struct Question {
    question: String,
    #[serde(rename = "header")]
    _header: Option<String>,
    #[serde(rename = "multiSelect", default)]
    multi_select: bool,
    options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuestionOption {
    label: String,
    #[serde(rename = "description")]
    _description: Option<String>,
}

/// 将 UseTool 转换为 ChoicesData
fn use_tool_to_choices(tool: &UseTool) -> ChoicesData {
    let tool_id = if !tool.id.is_empty() {
        Some(tool.id.clone())
    } else {
        None
    };

    // 检测 AskUserQuestion 并转换成 choices
    if tool.name == "AskUserQuestion"
        && let Ok(input) = serde_json::from_value::<AskUserQuestionInput>(tool.input.clone())
        && let Some(first_q) = input.questions.first()
    {
        let options: Vec<String> = first_q.options.iter().map(|o| o.label.clone()).collect();

        return ChoicesData {
            id: tool_id,
            title: first_q.question.clone(),
            options,
            multi_select: first_q.multi_select,
            allow_custom_input: true,
        };
    }

    // 其他工具，显示基本信息
    let title = match &tool.input {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let tool_name = format!("\x1b[96mTool: {}\x1b[30m", tool.name);
            let mut title_str = vec![tool_name];
            for k in keys {
                let v = &map[k];
                let value_str = match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    serde_json::Value::Null => "null".to_string(),
                    serde_json::Value::Array(arr) => serde_json::to_string(arr).unwrap_or_default(),
                    serde_json::Value::Object(obj) => {
                        serde_json::to_string(obj).unwrap_or_default()
                    }
                };
                title_str.push(format!("{}: {}", k, value_str));
            }
            title_str.join("\n\n")
        }
        _ => format!(
            "\x1b[35m{}\x1b[30m",
            serde_json::to_string_pretty(&tool.input)
                .unwrap_or(format!("Tool call: {:?}", tool.name))
        ),
    };

    ChoicesData {
        id: tool_id,
        title,
        options: vec![],
        multi_select: false,
        allow_custom_input: false,
    }
}

/// 根据终端状态生成要发送的消息
fn state_to_message(state: &ClaudeCodeState, session_id: &str) -> Option<ServerMessage> {
    match state {
        ClaudeCodeState::PreUseTool {
            request,
            is_pending,
            ..
        } => {
            if *is_pending {
                for r in request {
                    if r.done {
                        continue;
                    }

                    log::info!(
                        "[{}] Claude is requesting to use a tool: {:?}",
                        session_id,
                        r
                    );

                    let choice_data = use_tool_to_choices(r);
                    return Some(ServerMessage::Choices(choice_data));
                }
            }
            None
        }
        ClaudeCodeState::Output {
            output,
            is_thinking,
        } => {
            if *is_thinking {
                log::info!("[{}] Claude is thinking...", session_id);
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Info,
                    format!("\x1b[38;5;245m{output}\x1b[0m",),
                ))
            } else {
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Success,
                    output.clone(),
                ))
            }
        }
        ClaudeCodeState::StopUseTool { is_error } => {
            if *is_error {
                log::error!("[{}] Tool execution error", session_id);
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Error,
                    "Tool execution failed.".to_string(),
                ))
            } else {
                log::info!("[{}] Tool execution completed", session_id);
                Some(ServerMessage::coustom_notification(
                    String::new(),
                    None,
                    0x3CB371,
                ))
            }
        }
        ClaudeCodeState::Working { prompt } => Some(ServerMessage::notification(
            crate::protocol::NotificationLevel::Success,
            prompt.clone(),
        )),
        ClaudeCodeState::Idle => Some(ServerMessage::get_input(
            "Claude is waiting for user input...".to_string(),
        )),
    }
}

type ServerTx = broadcast::Sender<ServerMessage>;

type ClientTx = mpsc::Sender<ClientMessage>;
type ClientRx = mpsc::Receiver<ClientMessage>;

#[derive(Clone)]
pub struct AppState {
    pub tx: ServerTx,
    pub cli_tx: ClientTx,
}

fn t2s<S: AsRef<str>>(s: S) -> String {
    let s = hanconv::tw2sp(s.as_ref());
    s.replace("幺", "么")
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ClaudeMode {
    Normal,
    Plan,
    AcceptEdits,
}

impl std::fmt::Display for ClaudeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaudeMode::Normal => write!(f, "normal"),
            ClaudeMode::Plan => write!(f, "plan"),
            ClaudeMode::AcceptEdits => write!(f, "accept_edits"),
        }
    }
}

/// 会话状态，包含所有可扩展的状态标记
#[derive(Debug)]
struct SessionState {
    mode: ClaudeMode,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            mode: ClaudeMode::Normal,
        }
    }
}

impl SessionState {
    fn to_state_string(&self) -> String {
        let mut result = String::new();
        // mode 状态
        match self.mode {
            ClaudeMode::Normal => result.push_str("[N]"),
            ClaudeMode::Plan => result.push_str("[P]"),
            ClaudeMode::AcceptEdits => result.push_str("[E]"),
        }
        result
    }
}

pub enum RunCommandResult {
    Done,
    ChangeDir(String, ClientRx, mpsc::Receiver<crate::ui::UIEvent>),
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    command: Vec<String>,
    asr_config: AsrConfig,
    mut rx: ClientRx,
    mut ui_rx: mpsc::Receiver<crate::ui::UIEvent>,
    tx: ServerTx,
    pty_output_tx: tokio::sync::mpsc::Sender<bytes::Bytes>,
    current_dir: Option<std::path::PathBuf>,
    listen_port: u16,
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

        ClaudeResult(Box<ClaudeCodeResult>),

        Error,
    }

    let asr_client = reqwest::Client::new();

    let mut terminal = crate::terminal::claude::new_with_command(
        command.first().unwrap().as_str(),
        &command[1..],
        &[("VIBETTY_PORT".to_string(), listen_port.to_string())],
        (24, 80),
        current_dir,
    )
    .await?;

    let mut input_received = false;
    let mut session_state = SessionState::default();
    let mut wav_buffer = Vec::new();
    let mut wav_sample_rate = 16000;

    loop {
        let terminal_read_event = terminal.read_pty_output_and_history_line();

        let event = tokio::select! {
            result = terminal_read_event => {
                match result {
                    Ok(r) => TerminalEvent::ClaudeResult(Box::new(r)),
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
            }
        };

        match event {
            TerminalEvent::ClaudeResult(r)
                if matches!(r.as_ref(), ClaudeCodeResult::PtyOutput(_)) =>
            {
                let ClaudeCodeResult::PtyOutput(output) = *r else {
                    unreachable!()
                };
                log::trace!("[{}] PTY output: {}", terminal.session_id(), output.len());
                if output.contains("accept edits on") {
                    log::info!("[{}] Detected 'accept edits on'", terminal.session_id());
                    if session_state.mode != ClaudeMode::AcceptEdits {
                        session_state.mode = ClaudeMode::AcceptEdits;
                        let _ = tx.send(ServerMessage::status(session_state.to_state_string()));
                    }
                } else if output.contains("plan mode on") {
                    log::info!("[{}] Detected 'plan mode on'", terminal.session_id());
                    if session_state.mode != ClaudeMode::Plan {
                        session_state.mode = ClaudeMode::Plan;
                        let _ = tx.send(ServerMessage::status(session_state.to_state_string()));
                    }
                } else if output.contains("? for shortcuts") {
                    log::info!("[{}] Detected 'normal mode on'", terminal.session_id());
                    if session_state.mode != ClaudeMode::Normal {
                        session_state.mode = ClaudeMode::Normal;
                        let _ = tx.send(ServerMessage::status(session_state.to_state_string()));
                    }
                }

                if pty_output_tx
                    .send(bytes::Bytes::from(output.clone()))
                    .await
                    .is_err()
                {
                    log::warn!("[{}] No active PTY output receiver", terminal.session_id());
                    return Ok(RunCommandResult::Done);
                }

                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }
            }

            TerminalEvent::ClaudeResult(r)
                if matches!(r.as_ref(), ClaudeCodeResult::WaitForUserInput) =>
            {
                log::info!("[{}] Waiting for user input", terminal.session_id());

                terminal.update_state(&r);

                if let Err(_e) = tx.send(ServerMessage::get_input(
                    "Claude is waiting for user input...".to_string(),
                )) {
                    log::error!("[{}] No client waiting for data", terminal.session_id());
                }

                if input_received {
                    terminal.send_enter().await?;
                }
            }
            TerminalEvent::ClaudeResult(r) => {
                input_received = false;
                if terminal.update_state(&r) {
                    log::info!(
                        "[{}] Terminal state updated: {:?}",
                        terminal.session_id(),
                        terminal.state()
                    );
                    if let Some(msg) =
                        state_to_message(terminal.state(), &terminal.session_id().to_string())
                        && let Err(_e) = tx.send(msg)
                    {
                        log::error!("[{}] No client waiting for data", terminal.session_id());
                    }

                    // Handle Idle state input_received separately
                    match terminal.state() {
                        ClaudeCodeState::Idle => {
                            if input_received {
                                terminal.send_enter().await?;
                            }
                        }
                        ClaudeCodeState::Working { .. } => {
                            input_received = false;
                        }
                        _ => {}
                    }
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(input)) => {
                terminal.send_bytes(&input).await?;
                if input == b"\x1b[5~" || input == b"\x1b[6~" {
                    log::debug!("Received Page Up/Down input from UI");
                    pty_output_tx.send(bytes::Bytes::from_owner(input)).await?;
                    continue;
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Title(title)) => {
                log::debug!(
                    "[{}] Terminal title updated: {:?}",
                    terminal.session_id(),
                    title
                );
                if let Some(r) = terminal.update_title(title) {
                    log::info!(
                        "[{}] Terminal state updated from title: {:?}",
                        terminal.session_id(),
                        terminal.state()
                    );

                    if terminal.update_state(&r)
                        && let Some(msg) =
                            state_to_message(terminal.state(), &terminal.session_id().to_string())
                        && let Err(_e) = tx.send(msg)
                    {
                        log::error!("[{}] No client waiting for data", terminal.session_id());
                    }
                }
            }
            TerminalEvent::Input(ClientMessage::Sync) => {
                log::info!("Received Sync message from client");
                if let Some(msg) =
                    state_to_message(terminal.state(), &terminal.session_id().to_string())
                    && let Err(_e) = tx.send(msg)
                {
                    log::error!("[{}] No client waiting for data", terminal.session_id());
                }
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
                input_received = true;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                terminal.send_enter().await?;
            }
            TerminalEvent::Input(ClientMessage::Choice { index }) => {
                log::info!("Sending choice input to terminal: {:?}", index);
                if index < 0 {
                    terminal.send_esc().await?;
                    continue;
                }

                terminal.send_key_iter(&[(index + 1).to_string()]).await?;
            }
            TerminalEvent::Input(ClientMessage::Choices {
                index,
                custom_input,
                multi_select,
            }) => {
                log::info!("Sending choice input to terminal: {:?}", index);
                let mut keys = Vec::new();
                for i in index {
                    if i < 0 {
                        terminal.send_esc().await?;
                        continue;
                    }
                    keys.push((i + 1).to_string());
                }

                if let Some(input) = custom_input
                    && !input.trim().is_empty()
                {
                    terminal.send_up_arrow().await?;
                    log::info!("Sending custom input to terminal: {:?}", input);
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    terminal.send_text(&input).await?;
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    if multi_select {
                        terminal.send_up_arrow().await?;
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    } else {
                        terminal.send_enter().await?;
                        continue;
                    }
                }

                log::info!("Submitting choice input");

                terminal.send_key_iter(&keys).await?;

                if multi_select {
                    terminal.send_right_arrow().await?;
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    terminal.send_enter().await?;
                }
            }
            TerminalEvent::Input(ClientMessage::ChangeDir(path)) => {
                log::info!("Change directory requested: {}", path);
                let _ = tx.send(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Info,
                    format!("Changing directory to: {}", path),
                ));
                return Ok(RunCommandResult::ChangeDir(path, rx, ui_rx));
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
                let config = crate::util::WavConfig {
                    sample_rate: wav_sample_rate,
                    channels: 1,
                    bits_per_sample: 16,
                };
                let wav_data = crate::util::pcm_to_wav(&wav_buffer, config);

                let AsrConfig::Whisper(asr_config) = &asr_config;

                let r = retry_whisper(
                    &asr_client,
                    &asr_config.url,
                    &asr_config.api_key,
                    &asr_config.model,
                    &asr_config.lang,
                    &asr_config.prompt,
                    wav_data,
                    3,
                    std::time::Duration::from_secs(5),
                )
                .await;

                let mut asr_text = t2s(r.join("\n"));

                // 如果 ASR 结果等于环境变量 VIBETTY_EXIT_COMMAND 的值，替换为 "/exit"（大小写不敏感）
                if let Ok(exit_trigger) = std::env::var("VIBETTY_EXIT_COMMAND")
                    && asr_text.trim().to_lowercase() == exit_trigger.trim().to_lowercase()
                {
                    asr_text = "/exit".to_string();
                }

                let asr_result = format!("{} ", asr_text);

                if let Err(_e) = tx.send(ServerMessage::asr_result(asr_result)) {
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
                            log::info!("Received binary message, length: {}", data.len());
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
        let r = tokio::time::timeout(
            timeout,
            crate::asr::whisper(client, url, api_key, model, lang, prompt, wav_audio.clone()),
        )
        .await;
        match r {
            Ok(Ok(v)) => return v,
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
