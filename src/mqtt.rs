//! 可选的 MQTT 传输桥接。
//!
//! 当 `~/.vibetty/config.toml` 含 `[mqtt]` 段时,main.rs 调用 [`spawn`] 启动本模块。
//! 它是 WebSocket(`handle_socket`)之外的“另一个前端”:后端的 `cli_tx` / broadcast `tx`
//! 通道、PTY 逻辑全部复用。ESP32 端无需 msgpack——
//! - 原始按键 / PTY 输出走 raw 字节 topic(`pty_in` / `pty_out`);
//! - 控制类消息(输入文本、同步、滚动)合并到一个 `control` topic,payload 是
//!   `ClientMessage` 的 serde JSON,靠 `type` 字段区分。
//!
//! 只桥接终端核心消息;`notification`/`title` 不走 MQTT(WebSocket 端照常,ESP32 不需要)。
//!
//! ## Topic 约定(`{p}` = 实例前缀,构造见下方「Topic 命名 + 服务发现」)
//! 入站(ESP32 -> vibetty,vibetty 订阅):
//! - `{p}/pty_in`   原始按键字节(二进制)  -> `PtyInput`
//! - `{p}/control`  控制类消息(JSON)      -> 见下方 [`parse_control`]
//!
//!   `control` payload 是 `ClientMessage` 的 serde JSON(`{"type":...,"data":...}`):
//!   `type` ∈ `input_text`(data=字符串)、`sync` / `scroll_up` / `scroll_down`(无 data)。
//!   原始按键走 `pty_in`,不在此 topic。
//!
//! 出站(vibetty -> ESP32,vibetty 发布):
//! - `{p}/pty_out`  PTY 原始输出字节  <- `PtyOutput`
//! - `{p}/screen`   整张 PNG/JPEG 字节 <- `Screen`(格式靠 magic bytes 区分)
//!
//! ## Topic 命名 + 服务发现(多实例)
//! 每个实例的 topic 前缀自动构造为 `{user}/{device}/{pid}/vibetty`:
//! - `user`   = `[mqtt] username`(配了 broker 登录名),没配则回退 `device`;
//! - `device` = SHA256(machine-uid) 前 16 hex(设备指纹,稳定 + 跨机器唯一,不泄露原始机器 ID);
//! - `pid`    = 进程 pid(区分同一台机器上同时跑的多个 vibetty;跨重启会变)。
//!
//! 数据 topic 挂在前缀下:`{prefix}/pty_in`、`/pty_out`、`/control`、`/screen`。
//! 实例上线时在 `{prefix}` 发一条 retained presence(`{prefix,client_id,ts}`),每 15s 重发(心跳);
//! LWT 在异常掉线时清空它。ESP32 订阅 `{user}/+/+/vibetty` 即可发现该用户的所有实例
//! (通配 device 与 pid),再按 prefix 精确订阅数据通道。

use bytes::Bytes;
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, Transport};
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

/// 当前 epoch 秒,作为 presence 公告的 `ts`(ESP32 据此判断心跳超时)。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// presence 公告 JSON:声明本实例存在,含 `prefix`(ESP32 据此控制)+ `client_id` + `ts`。
fn presence_payload(prefix: &str, client_id: &str) -> String {
    serde_json::json!({
        "prefix": prefix,
        "client_id": client_id,
        "ts": now_secs(),
    })
    .to_string()
}

/// 设备指纹:SHA256(machine-uid) 前 16 hex。稳定(跨重启)+ 唯一(跨机器),
/// 且不泄露原始机器 ID。读取失败回退 "unknown"(极端情况,不影响其余实例隔离)。
fn device_hash() -> String {
    use sha2::{Digest, Sha256};
    let raw = machine_uid::get().unwrap_or_else(|_| "unknown".to_string());
    Sha256::digest(raw.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// 把任意字符串清理成合法的 MQTT topic 单段(不含 `/`,只留字母数字与 `.` `_` `-`)。
fn sanitize_segment(s: &str) -> String {
    let clean: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "x".to_string()
    } else {
        clean
    }
}

/// 实例 topic 前缀 `{user}/{device}/{pid}/vibetty`。
/// - `user`:配了 `[mqtt] username` 用它(多租户隔离);没配则回退 `root`。
/// - `device`:设备指纹,定位机器。
/// - `pid`:区分同一台机器上同时跑的多个 vibetty(跨重启变,ESP32 用 `+` 通配订阅)。
fn instance_prefix(username: Option<&str>, device: &str) -> String {
    let user = username
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(sanitize_segment)
        .unwrap_or_else(|| "root".to_string());
    format!("{user}/{device}/{pid}/vibetty", pid = std::process::id())
}

/// 入站 topic 后缀(ESP32 -> vibetty)。payload -> `ClientMessage` 的映射在 `run_bridge`。
const INBOUND_TOPICS: &[(&str, u8)] = &[("pty_in", 0), ("control", 1)];

/// presence 公告(心跳)重发间隔;ESP32 靠 payload 的 `ts` 判断实例是否存活。
const PRESENCE_INTERVAL_SECS: u64 = 15;

/// 解析 `{prefix}/control` 的 JSON payload。复用 `ClientMessage` 的 serde 形式
/// (`{"type":"input_text","data":"ls"}` 等),只接受控制类消息(input/sync/scroll_*);
/// 原始按键(`pty_in`)与语音类在此 topic 上忽略并告警。
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

/// 从 broker URL(`mqtt://user:pass@host:port`)解析出的连接信息。
struct ParsedBroker {
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    use_tls: bool,
}

/// 解析 `mqtt://[user[:pass]@]host[:port]` / `mqtts://...`。
/// 无账号时 username/password 为 None(匿名连,用于内置 broker 等)。
fn parse_broker_url(url: &str) -> anyhow::Result<ParsedBroker> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("MQTT URL missing '://': {url}"))?;
    let use_tls = match scheme {
        "mqtt" => false,
        "mqtts" => true,
        other => anyhow::bail!("Unsupported MQTT scheme '{other}', use mqtt:// or mqtts://"),
    };
    let (userinfo, hostport) = match rest.rfind('@') {
        Some(idx) => (Some(&rest[..idx]), &rest[idx + 1..]),
        None => (None, rest),
    };
    let hostport = hostport.split('/').next().unwrap_or(hostport);
    let (username, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((user, pass)) => (Some(user.to_string()), Some(pass.to_string())),
            None => (Some(u.to_string()), None),
        },
        None => (None, None), // 匿名
    };
    let default_port = if use_tls { 8883 } else { 1883 };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(default_port)),
        None => (hostport.to_string(), default_port),
    };
    if host.is_empty() {
        anyhow::bail!("MQTT URL missing host");
    }
    Ok(ParsedBroker {
        host,
        port,
        username,
        password,
        use_tls,
    })
}

async fn run_bridge(
    cfg: MqttConfig,
    cli_tx: mpsc::Sender<ClientMessage>,
    tx: broadcast::Sender<ServerMessage>,
    image_format: ImageFormat,
) {
    // broker 是完整 URL:mqtt(s)://[user:pass@]host[:port]。scheme 决定 TLS,
    // user/pass 在 URL 里;端口没写则按协议默认(mqtt 1883 / mqtts 8883)。
    let broker = match parse_broker_url(&cfg.broker) {
        Ok(b) => b,
        Err(e) => {
            log::error!("[mqtt] invalid broker url {:?}: {e}", cfg.broker);
            return;
        }
    };
    let device = device_hash();
    let prefix = instance_prefix(broker.username.as_deref(), &device);
    // client_id 带机器指纹 + pid:跨机器跑多个 vibetty(复用同一 config)也不会撞 client_id。
    let client_id = format!("vibetty-{device}-{}", std::process::id());

    let mut opts = MqttOptions::new(client_id.clone(), broker.host.clone(), broker.port);
    opts.set_keep_alive(std::time::Duration::from_secs(cfg.keep_alive_secs));
    // rumqttc 默认 max packet 10KB:screen PNG(~22KB)/ pty_out(可达几十 KB+)会超,
    // 触发 eventloop error → 断连重连 → LWT 覆盖 presence(页面就显示"实例下线")。
    // 调到 1MB(broker 端 max_payload_size 也要 >= 此值,见 broker.rs)。
    opts.set_max_packet_size(1024 * 1024, 1024 * 1024);
    // 遗嘱:异常掉线 → broker 自动发空 retained → 清掉本实例的 presence 公告。
    opts.set_last_will(LastWill {
        topic: prefix.clone(),
        message: Bytes::new(),
        qos: QoS::AtLeastOnce,
        retain: true,
    });
    if broker.use_tls {
        opts.set_transport(Transport::tls_with_default_config());
    }
    if let (Some(u), Some(p)) = (&broker.username, &broker.password) {
        opts.set_credentials(u.clone(), p.clone());
    }

    let (client, mut eventloop) = AsyncClient::new(opts, 50);

    for (name, qos) in INBOUND_TOPICS {
        if let Err(e) = client
            .subscribe(format!("{prefix}/{name}"), qos_from_u8(*qos))
            .await
        {
            log::error!("[mqtt] subscribe {prefix}/{name} failed: {e}");
            return;
        }
    }

    // 上线公告:在本实例前缀 topic 发 retained,声明存在。
    // ESP32 订阅 `{user}/+/+/vibetty` 即可发现该用户所有实例(retained 保证新订阅立即收到)。
    let presence = presence_payload(&prefix, &client_id);
    if let Err(e) = client
        .publish(prefix.clone(), QoS::AtLeastOnce, true, presence)
        .await
    {
        log::warn!("[mqtt] initial presence publish failed: {e}");
    }

    // 心跳:定期重发 presence 更新 ts,让 ESP32 判断存活。
    // 进程退出 → 心跳停 → ts 停止更新 → ESP32 超时判定离线(LWT 兜底异常断连)。
    let hb_client = client.clone();
    let hb_topic = prefix.clone();
    let hb_prefix = prefix.clone();
    let hb_client_id = client_id.clone();
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(PRESENCE_INTERVAL_SECS));
        interval.tick().await; // 跳过首次(上线公告刚发过)
        loop {
            interval.tick().await;
            let payload = presence_payload(&hb_prefix, &hb_client_id);
            if let Err(e) = hb_client
                .publish(&hb_topic, QoS::AtLeastOnce, true, payload)
                .await
            {
                log::warn!("[mqtt] presence heartbeat failed: {e}");
            }
        }
    });

    log::info!(
        "[mqtt] bridging: prefix={prefix} tls={} (pty_in raw + control JSON, no msgpack)",
        broker.use_tls
    );

    // 出站任务:订阅 broadcast,只转发 Screen(整张图),其余忽略。
    // 暂时停发 pty_out(调试期只发 screen PNG);恢复时取消下面注释即可。
    // let pty_out_topic = format!("{prefix}/pty_out");
    let screen_topic = format!("{prefix}/screen");
    let mut rx = tx.subscribe();
    let pub_client = client.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ServerMessage::PtyOutput(bytes)) => {
                    // 暂时不往 MQTT 发 pty_out(调试期只发 screen PNG)。
                    log::debug!("[mqtt] pty_out skipped ({} bytes)", bytes.len());
                }
                Ok(ServerMessage::Screen(screen)) => {
                    let mut h = 0u16;
                    match render_screen_to_image(screen.as_ref(), None, &mut h, image_format) {
                        Ok(img) if !img.is_empty() => {
                            log::debug!(
                                "[mqtt] screen image {} bytes -> {screen_topic}",
                                img.len()
                            );
                            if let Err(e) = pub_client
                                .publish(&screen_topic, QoS::AtMostOnce, true, img)
                                .await
                            {
                                log::warn!("[mqtt] publish screen failed: {e}");
                            }
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("[mqtt] render screen failed: {e}"),
                    }
                }
                Ok(_) => {
                    // Notification/Title/ScreenImage 不走 MQTT(ESP32 不需要)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_with_credentials() {
        let b = parse_broker_url("mqtt://alice:secret@broker.example.com:1883").unwrap();
        assert_eq!(b.host, "broker.example.com");
        assert_eq!(b.port, 1883);
        assert_eq!(b.username.as_deref(), Some("alice"));
        assert_eq!(b.password.as_deref(), Some("secret"));
        assert!(!b.use_tls);
    }

    #[test]
    fn parse_url_anonymous() {
        // 无账号(匿名 URL):username/password 为 None
        let b = parse_broker_url("mqtt://192.168.1.10:1883").unwrap();
        assert_eq!(b.host, "192.168.1.10");
        assert_eq!(b.port, 1883);
        assert!(b.username.is_none());
        assert!(b.password.is_none());
        assert!(!b.use_tls);
    }

    #[test]
    fn parse_url_mqtts_default_port() {
        let b = parse_broker_url("mqtts://bob:p@host.io").unwrap();
        assert_eq!(b.port, 8883);
        assert!(b.use_tls);
    }

    #[test]
    fn parse_url_no_port() {
        let b = parse_broker_url("mqtt://broker.io").unwrap();
        assert_eq!(b.port, 1883);
    }
}
