//! 可选的 MQTT 传输桥接。
//!
//! 当 `~/.vibetty/config.toml` 含 `[mqtt]` 段时,main.rs 调用 [`spawn`] 启动本模块。
//! 它是 WebSocket(`handle_socket`)之外的“另一个前端”:后端的 `cli_tx` / broadcast `tx`
//! 通道、PTY 逻辑全部复用。ESP32 端无需 msgpack——
//! - 原始按键 / PTY 输出走 raw 字节 topic(`pty_in` / `pty_out`);
//! - 控制类消息(输入文本、切目录、同步、滚动)合并到一个 `control` topic,payload 是
//!   `ClientMessage` 的 serde JSON,靠 `type` 字段区分。
//!
//! 只桥接终端核心消息;`notification`/`asr_result`/`title`/`voice_*` 不走 MQTT
//! (WebSocket 端照常,ESP32 不需要)。
//!
//! ## Topic 约定(`{topic_prefix}` 来自 `[mqtt]`)
//! 入站(ESP32 -> vibetty,vibetty 订阅):
//! - `{p}/pty_in`   原始按键字节(二进制)  -> `PtyInput`
//! - `{p}/control`  控制类消息(JSON)      -> 见下方 [`parse_control`]
//!
//!   `control` payload 是 `ClientMessage` 的 serde JSON(`{"type":...,"data":...}`):
//!   `type` ∈ `input_text`(data=字符串)、`change_dir`(data=路径)、
//!   `sync` / `scroll_up` / `scroll_down`(无 data)。原始按键走 `pty_in`,不在此 topic。
//!
//! 出站(vibetty -> ESP32,vibetty 发布):
//! - `{p}/pty_out`  PTY 原始输出字节  <- `PtyOutput`
//! - `{p}/screen`   整张 PNG/JPEG 字节 <- `Screen`(格式靠 magic bytes 区分)

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, Transport};
use tokio::sync::{broadcast, mpsc};

use crate::config::MqttConfig;
use crate::protocol::{ClientMessage, ImageFormat, ServerMessage};
use crate::ws::render_screen_to_image;

/// 启动 MQTT 桥接(后台任务)。main.rs 在解析到 `MqttConfig` 后调用。
///
/// - `cli_tx`: 客户端消息进入 PTY 会话的入口(与 `AppState.cli_tx` 同一个)
/// - `tx`:     服务端广播源(与 `AppState.tx` 同一个),内部 `.subscribe()` 取副本
pub fn spawn(
    cfg: MqttConfig,
    cli_tx: mpsc::Sender<ClientMessage>,
    tx: broadcast::Sender<ServerMessage>,
    image_format: ImageFormat,
) {
    tokio::spawn(run_bridge(cfg, cli_tx, tx, image_format));
}

fn qos_from_u8(q: u8) -> QoS {
    match q {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

/// 入站 topic 后缀(ESP32 -> vibetty)。payload -> `ClientMessage` 的映射在 `run_bridge`。
const INBOUND_TOPICS: &[&str] = &["pty_in", "control"];

/// 解析 `{prefix}/control` 的 JSON payload。复用 `ClientMessage` 的 serde 形式
/// (`{"type":"input_text","data":"ls"}` 等),只接受控制类消息(input/change_dir/
/// sync/scroll_*);原始按键(`pty_in`)与语音类在此 topic 上忽略并告警。
fn parse_control(payload: &[u8]) -> Option<ClientMessage> {
    let cm = match serde_json::from_slice::<ClientMessage>(payload) {
        Ok(cm) => cm,
        Err(e) => {
            log::warn!("[mqtt] control topic invalid JSON: {e}");
            return None;
        }
    };
    if matches!(
        cm,
        ClientMessage::Input(_)
            | ClientMessage::ChangeDir(_)
            | ClientMessage::Sync
            | ClientMessage::ScrollUp
            | ClientMessage::ScrollDown
    ) {
        Some(cm)
    } else {
        log::warn!(
            "[mqtt] control topic ignores {:?} (raw keys belong on pty_in)",
            cm
        );
        None
    }
}

async fn run_bridge(
    cfg: MqttConfig,
    cli_tx: mpsc::Sender<ClientMessage>,
    tx: broadcast::Sender<ServerMessage>,
    image_format: ImageFormat,
) {
    let prefix = cfg.topic_prefix.clone();
    let mut opts = MqttOptions::new(cfg.effective_client_id(), cfg.host.clone(), cfg.port);
    opts.set_keep_alive(std::time::Duration::from_secs(cfg.keep_alive_secs));
    if cfg.effective_use_tls() {
        opts.set_transport(Transport::tls_with_default_config());
    }
    if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
        opts.set_credentials(u.clone(), p.clone());
    }

    let (client, mut eventloop) = AsyncClient::new(opts, 50);
    let qos = qos_from_u8(cfg.qos);

    for name in INBOUND_TOPICS {
        if let Err(e) = client.subscribe(format!("{prefix}/{name}"), qos).await {
            log::error!("[mqtt] subscribe {prefix}/{name} failed: {e}");
            return;
        }
    }
    log::info!(
        "[mqtt] bridging: prefix={prefix} qos={qos:?} tls={} (pty_in raw + control JSON, no msgpack)",
        cfg.effective_use_tls()
    );

    // 出站任务:订阅 broadcast,只转发 PtyOutput 和 Screen(整张图),其余忽略。
    let pty_out_topic = format!("{prefix}/pty_out");
    let screen_topic = format!("{prefix}/screen");
    let mut rx = tx.subscribe();
    let pub_client = client.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ServerMessage::PtyOutput(bytes)) => {
                    if let Err(e) = pub_client.publish(&pty_out_topic, qos, false, bytes).await {
                        log::warn!("[mqtt] publish pty_out failed: {e}");
                    }
                }
                Ok(ServerMessage::Screen(screen)) => {
                    let mut h = 0u16;
                    match render_screen_to_image(screen.as_ref(), None, &mut h, image_format) {
                        Ok(img) if !img.is_empty() => {
                            if let Err(e) = pub_client.publish(&screen_topic, qos, false, img).await
                            {
                                log::warn!("[mqtt] publish screen failed: {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("[mqtt] render screen failed: {e}"),
                    }
                }
                Ok(_) => {
                    // Notification/AsrResult/Title/ScreenImage 不走 MQTT(ESP32 不需要)
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("[mqtt] broadcast lagged {n} messages");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    log::info!("[mqtt] broadcast closed, stop outbound");
                    break;
                }
            }
        }
    });

    // 入站:poll eventloop,按 topic 后缀构造 ClientMessage -> cli_tx
    let pfx = format!("{prefix}/");
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let suffix = p.topic.strip_prefix(&pfx).unwrap_or("");
                let payload = p.payload.to_vec();
                let msg = match suffix {
                    "pty_in" => Some(ClientMessage::PtyInput(payload)),
                    "control" => parse_control(&payload),
                    _ => None,
                };
                if let Some(cm) = msg
                    && let Err(e) = cli_tx.send(cm).await
                {
                    log::error!("[mqtt] cli_tx send failed: {e}");
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => {
                // rumqttc EventLoop 自带自动重连;这里只记录并退避,WS/PTY 不受影响。
                log::warn!("[mqtt] eventloop error (auto-reconnecting): {e}");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}
