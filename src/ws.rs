use std::collections::HashMap;

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use echokit_terminal::terminal::claude::ClaudeCodeResult;
use tokio::sync::{broadcast, mpsc};

use crate::{
    config::AsrConfig,
    protocol::{ChoicesData, ClientMessage, ServerMessage, VoiceInputStart},
};

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

    loop {
        let event = tokio::select! {
            result = terminal.read_pty_output_and_history_line() => {
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

        if !matches!(
            event,
            TerminalEvent::ClaudeResult(ClaudeCodeResult::WaitForUserInput)
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

                let _ = tx.send(ServerMessage::get_input(
                    "Claude is waiting for user input...".to_string(),
                ));

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
                    match terminal.state() {
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
                                        terminal.session_id(),
                                        r
                                    );

                                    let title = match serde_json::from_value::<
                                        HashMap<String, String>,
                                    >(
                                        r.input.clone()
                                    ) {
                                        Ok(map) => {
                                            let mut title_str = vec![format!("Tool: {}", r.name)];
                                            for (k, v) in map {
                                                title_str.push(format!("{}: {}", k, v));
                                            }
                                            title_str.join(", ")
                                        }
                                        Err(_) => serde_json::to_string_pretty(&r.input)
                                            .unwrap_or(format!("Tool call: {:?}", r.name)),
                                    };

                                    let choice_data = ChoicesData {
                                        title,
                                        options: vec![],
                                    };

                                    let _ = tx.send(ServerMessage::Choices(choice_data));
                                    break;
                                }
                            }
                        }
                        echokit_terminal::terminal::claude::ClaudeCodeState::Output {
                            output,
                            is_thinking,
                        } => {
                            if *is_thinking {
                                log::info!("[{}] Claude is thinking...", terminal.session_id());
                                let _ = tx.send(ServerMessage::notification(
                                    crate::protocol::NotificationLevel::Info,
                                    "Claude is thinking...".to_string(),
                                ));
                            } else {
                                let _ = tx.send(ServerMessage::screen_text(output.clone()));
                            }
                        }
                        echokit_terminal::terminal::claude::ClaudeCodeState::StopUseTool {
                            is_error,
                        } => {
                            if *is_error {
                                log::error!("[{}] Tool execution error", terminal.session_id());
                                let _ = tx.send(ServerMessage::notification(
                                    crate::protocol::NotificationLevel::Error,
                                    "Tool execution failed. Please try again.".to_string(),
                                ));
                            } else {
                                log::info!("[{}] Tool execution completed", terminal.session_id());
                                let _ = tx.send(ServerMessage::notification(
                                    crate::protocol::NotificationLevel::Info,
                                    "Tool execution completed successfully.".to_string(),
                                ));
                            }
                        }
                        echokit_terminal::terminal::claude::ClaudeCodeState::Idle => {
                            let _ = tx.send(ServerMessage::get_input(
                                "Claude is waiting for user input...".to_string(),
                            ));

                            if input_received {
                                terminal.send_enter().await?;
                            }
                        }
                    }
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

                let _ = tx.send(ServerMessage::asr_result(asr_result));
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

    loop {
        tokio::select! {
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
