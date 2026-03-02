use std::collections::HashMap;

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use echokit_terminal::terminal::claude::{ClaudeCodeResult, UseTool};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};

use crate::{
    config::AsrConfig,
    protocol::{ChoicesData, ClientMessage, ServerMessage, VoiceInputStart},
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
    #[serde(rename = "multiSelect")]
    _multi_select: Option<bool>,
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
    // 检测 AskUserQuestion 并转换成 choices
    if tool.name == "AskUserQuestion" {
        if let Ok(input) = serde_json::from_value::<AskUserQuestionInput>(tool.input.clone()) {
            if let Some(first_q) = input.questions.first() {
                let options: Vec<String> =
                    first_q.options.iter().map(|o| o.label.clone()).collect();

                return ChoicesData {
                    title: first_q.question.clone(),
                    options,
                };
            }
        }
    }

    // 其他工具，显示基本信息
    let title = match serde_json::from_value::<HashMap<String, String>>(tool.input.clone()) {
        Ok(map) => {
            let mut title_str = vec![format!("Tool: {}", tool.name)];
            for (k, v) in map {
                title_str.push(format!("{}: {}", k, v));
            }
            title_str.join("\n\n")
        }
        Err(_) => serde_json::to_string_pretty(&tool.input)
            .unwrap_or(format!("Tool call: {:?}", tool.name)),
    };

    ChoicesData {
        title,
        options: vec![],
    }
}

/// 根据终端状态生成要发送的消息
fn state_to_message(
    state: &echokit_terminal::terminal::claude::ClaudeCodeState,
    session_id: &str,
) -> Option<ServerMessage> {
    match state {
        echokit_terminal::terminal::claude::ClaudeCodeState::PreUseTool {
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
        echokit_terminal::terminal::claude::ClaudeCodeState::Output {
            output,
            is_thinking,
        } => {
            if *is_thinking {
                log::info!("[{}] Claude is thinking...", session_id);
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Info,
                    format!("Claude is thinking...\n {output}",),
                ))
            } else {
                Some(ServerMessage::screen_text(output.clone()))
            }
        }
        echokit_terminal::terminal::claude::ClaudeCodeState::StopUseTool { is_error } => {
            if *is_error {
                log::error!("[{}] Tool execution error", session_id);
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Error,
                    "Tool execution failed. Please try again.".to_string(),
                ))
            } else {
                log::info!("[{}] Tool execution completed", session_id);
                Some(ServerMessage::notification(
                    crate::protocol::NotificationLevel::Info,
                    "Tool execution completed successfully.".to_string(),
                ))
            }
        }
        echokit_terminal::terminal::claude::ClaudeCodeState::Working { prompt } => {
            Some(ServerMessage::notification(
                crate::protocol::NotificationLevel::Success,
                prompt.clone(),
            ))
        }
        echokit_terminal::terminal::claude::ClaudeCodeState::Idle => Some(
            ServerMessage::get_input("Claude is waiting for user input...".to_string()),
        ),
        _ => None,
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
    s.replace("什幺", "什么")
}

pub async fn run_command(
    command: Vec<String>,
    asr_config: AsrConfig,
    mut rx: ClientRx,
    tx: ServerTx,
) -> anyhow::Result<()> {
    enum TerminalEvent {
        Input(crate::protocol::ClientMessage),
        InputClosed,

        ClaudeResult(ClaudeCodeResult),

        Error,
    }

    let asr_client = reqwest::Client::new();

    let mut terminal = echokit_terminal::terminal::claude::new_with_command(
        command.first().unwrap().as_str(),
        &command[1..],
        (24, 80),
    )
    .await?;

    let mut input_received = false;
    let mut wav_buffer = Vec::new();
    let mut wav_sample_rate = 16000;
    let mut no_ws_client = true;

    struct NeverReady;
    impl std::future::Future for NeverReady {
        type Output = ();

        fn poll(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Pending
        }
    }

    loop {
        let terminal_read_event = async {
            if no_ws_client {
                NeverReady.await;
                unreachable!();
            } else {
                terminal.read_pty_output_and_history_line().await
            }
        };

        let event = tokio::select! {
            result = terminal_read_event => {
                match result {
                    Ok(r) => TerminalEvent::ClaudeResult(r),
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
        };

        if matches!(
            event,
            TerminalEvent::ClaudeResult(ClaudeCodeResult::ClaudeLog(
                echokit_terminal::types::claude::ClaudeCodeLog::UserMessage(..)
            ))
        ) {
            input_received = false;
        }

        match event {
            TerminalEvent::ClaudeResult(ClaudeCodeResult::PtyOutput(output)) => {
                log::info!("[{}] PTY output: {}", terminal.session_id(), output.len());

                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }
            }

            TerminalEvent::ClaudeResult(ClaudeCodeResult::WaitForUserInput) => {
                log::info!("[{}] Waiting for user input", terminal.session_id());

                terminal.update_state(&ClaudeCodeResult::WaitForUserInput);

                if let Err(_e) = tx.send(ServerMessage::get_input(
                    "Claude is waiting for user input...".to_string(),
                )) {
                    log::error!("[{}] No client waiting for data", terminal.session_id());
                    no_ws_client = true;
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
                    {
                        if let Err(_e) = tx.send(msg) {
                            log::error!("[{}] No client waiting for data", terminal.session_id());
                            no_ws_client = true;
                        }
                    }

                    // Handle Idle state input_received separately
                    if matches!(
                        terminal.state(),
                        echokit_terminal::terminal::claude::ClaudeCodeState::Idle
                    ) && input_received
                    {
                        terminal.send_enter().await?;
                    }
                }
            }
            TerminalEvent::Input(ClientMessage::Sync) => {
                log::info!("Received Sync message from client");
                no_ws_client = false;
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

                for _ in 0..index {
                    terminal.send_down_arrow().await?;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }

                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                terminal.send_enter().await?;
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

                let asr_result = t2s(r.join("\n"));

                if let Err(_e) = tx.send(ServerMessage::asr_result(asr_result)) {
                    log::error!("[{}] No client waiting for data", terminal.session_id());
                    no_ws_client = true;
                }
            }
            TerminalEvent::InputClosed | TerminalEvent::Error => {
                log::error!("Input channel closed or error occurred, terminating terminal loop");
                break;
            }
        }
    }

    Ok(())
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut server_rx = state.tx.subscribe();
    if let Err(_) = state.cli_tx.send(ClientMessage::Sync).await {
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
                                    log::info!("Parsed client message: {:?}", client_msg);
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
