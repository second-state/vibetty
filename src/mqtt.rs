//! 可选的 MQTT 传输桥接。
//!
//! 当 `~/.vibetty/config.toml` 含 `[mqtt]` 段时,main.rs 调用 [`spawn`] 启动本模块。
//! 它是终端的传输通道:后端的 `cli_tx` / broadcast `tx` 通道、PTY 逻辑全部复用。
//! ESP32 端无需 msgpack——
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
//!   `type` ∈ `input_text`(data=字符串)、`sync`(data=`{width,height,pixels}`,
//!   `pixels=true`(默认)= 像素尺寸、`pixels=false` = 字符列/行;服务端换算成列/行后 resize PTY)、
//!   `scroll_up` / `scroll_down`(data=`{rows}`,`rows`=0/缺省=滚一整页)。原始按键走 `pty_in`,不在此 topic。
//!
//! 出站(vibetty -> ESP32,vibetty 发布):
//! - `{p}/screen`      整张 JPEG 字节(+末尾 4 字节 offset trailer)  <- `Screen`(`-q high/medium/low`)
//! - `{p}/screen_text` text 模式屏幕文本,首字节 tag:0x00=全屏基线 / 0x01=pty_out 增量  <- `Screen` + `PtyOutput`(`-q text`)
//!
//! ## Topic 命名 + 服务发现(多实例)
//! 每个实例的 topic 前缀自动构造为 `{user}/{device}/{pid}/vibetty`:
//! - `user`   = `[mqtt] username`(配了 broker 登录名),没配则回退 `device`;
//! - `device` = SHA256(machine-uid) 前 16 hex(设备指纹,稳定 + 跨机器唯一,不泄露原始机器 ID);
//! - `pid`    = 进程 pid(区分同一台机器上同时跑的多个 vibetty;跨重启会变)。
//!
//! 数据 topic 挂在前缀下:`{prefix}/pty_in`、`/pty_out`、`/control`、`/screen`。
//! 实例上线时在 `{prefix}` 发一条 retained presence(`{prefix,client_id,ts,title,state,format}`),每 15s 重发(心跳);
//! LWT 在异常掉线时清空它。ESP32 订阅 `{user}/+/+/vibetty` 即可发现该用户的所有实例
//! (通配 device 与 pid),再按 prefix 精确订阅数据通道。

use bytes::Bytes;
use rumqttc::{AsyncClient, Event, LastWill, MqttOptions, Packet, QoS, Transport};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::config::MqttConfig;
use crate::protocol::{ClientMessage, OutputFormat, ServerMessage};
use crate::terminal::agent::AgentState;
use crate::ws::{render_screen_to_image, render_screen_to_text};

/// 传输 client 的停止句柄:持有 oneshot 发送端。`stop()` 发信号让 `run_bridge`
/// 优雅退出(出站随 cancel 一同 break);连接断开后 broker 触发 LWT 清空 presence(实例下线)。
pub struct MqttHandle {
    cancel: oneshot::Sender<()>,
}

impl MqttHandle {
    /// 请求 client 停止(消耗 self)。重入安全:run_bridge 收到信号后自行 break + abort。
    pub fn stop(self) {
        let _ = self.cancel.send(());
    }
}

/// 启动 MQTT 桥接(后台任务),返回停止句柄。调用方按需 `.stop()`,或直接丢弃(运行到进程结束)。
///
/// - `cli_tx`: 客户端消息进入 PTY 会话的入口(与 `AppState.cli_tx` 同一个)
/// - `tx`:     服务端广播源(与 `AppState.tx` 同一个),内部 `.subscribe()` 取副本
pub fn spawn(
    cfg: MqttConfig,
    cli_tx: mpsc::Sender<ClientMessage>,
    tx: broadcast::Sender<ServerMessage>,
    image_format: OutputFormat,
) -> MqttHandle {
    let (cancel_tx, cancel_rx) = oneshot::channel();
    tokio::spawn(run_bridge(cfg, cli_tx, tx, image_format, cancel_rx));
    MqttHandle { cancel: cancel_tx }
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

/// presence 公告 JSON:`prefix`/`client_id`/`ts` 由 MQTT 端填,`title`/`state` 由 ws 随事件带来,
/// `format` 取自本实例的 `-q` 设置(ESP32 据此决定订阅 `{p}/screen` 还是 `{p}/screen_text`)。
fn presence_payload(
    prefix: &str,
    client_id: &str,
    title: &str,
    state: AgentState,
    format: OutputFormat,
) -> String {
    serde_json::json!({
        "prefix": prefix,
        "client_id": client_id,
        "ts": now_secs(),
        "title": title,
        "state": state,
        "format": format.as_str(),
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
pub const PRESENCE_INTERVAL_SECS: u64 = 15;

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
            | ClientMessage::Sync { .. }
            | ClientMessage::ScrollUp { .. }
            | ClientMessage::ScrollDown { .. }
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
    image_format: OutputFormat,
    mut cancel: oneshot::Receiver<()>,
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
    // 入站订阅不在启动时一次性发起,而是改到每次(重)连接的 CONNACK 之后发起
    // (见下方 select! 的 ConnAck 分支)。原因:网络断开重连后,broker 侧订阅会随
    // session 失效而丢失(clean session / broker session 过期),只在启动订阅一次
    // 会出现「重连成功、出站正常、入站收不到」。每次连接建立都重订阅,幂等且防御。

    // presence(上线公告 + 心跳 + 状态)统一由 ws 主循环发 ServerMessage::Presence 触发;
    // 本线程只负责收到后 publish 到 {prefix}(retained),不再自带定时心跳。
    log::info!(
        "[mqtt] bridging: prefix={prefix} tls={} (pty_in raw + control JSON, no msgpack)",
        broker.use_tls
    );

    // 出站:订阅 broadcast,转发 Screen(JPEG 或 text 全屏/增量)+ Presence(上线/心跳/状态)。
    let screen_topic = format!("{prefix}/screen");
    // text 模式屏幕文本走独立 topic,与 JPEG 的 /screen 分开。首字节 tag:0x00=全屏基线、
    // 0x01=pty_out 增量。**两种屏 topic(screen / screen_text 全屏+增量)都不 retained**:
    // pid 在前缀里,retained 会在重启后留在老 pid 的 topic 上清不掉(EMQX 累积);ESP32
    // 重连靠主动发 sync 拿首帧,不依赖 retained。presence 仍 retained(进程退出时 LWT 清)。
    let screen_text_topic = format!("{prefix}/screen_text");
    // 最近一次 sync 的客户端显示尺寸(px);出站渲染时据此把图片精确补齐到该尺寸。
    let mut sync_width: u16 = 0;
    let mut sync_height: u16 = 0;
    // 最近一次 presence 的 title/state;重连后用它补发上线公告(见 ConnAck 分支)。
    let mut last_presence: Option<(String, AgentState)> = None;
    // 累计发送的 screen image 字节数;每次 publish 时 log debug(单位 MB, f64)。
    let mut total_screen_bytes: u64 = 0;
    let mut rx = tx.subscribe();

    // 出站(rx 收 Screen → 渲染补齐 → publish;Presence → publish)+ 入站(poll eventloop
    // → ClientMessage → cli_tx)都在这一个 select! 里,同时监听 cancel:`MqttHandle::stop()`
    // 发信号即 break。**不调用 disconnect()**:直接 drop 连接,broker 视为异常掉线 → 必发 LWT 清 presence。
    let pfx = format!("{prefix}/");
    loop {
        tokio::select! {
            _ = &mut cancel => {
                log::info!("[mqtt] cancel signal received, stopping client");
                break;
            }
            ev = eventloop.poll() => match ev {
                Ok(Event::Incoming(Packet::Publish(p))) => {
                    let suffix = p.topic.strip_prefix(&pfx).unwrap_or("");
                    let payload = p.payload.to_vec();
                    let msg = match suffix {
                        "pty_in" => Some(ClientMessage::PtyInput(payload)),
                        "control" => parse_control(&payload),
                        _ => None,
                    };
                    if let Some(cm) = msg {
                        // 记下 sync 带的客户端尺寸,供出站渲染补齐图片到该【像素】尺寸用。
                        // pixels=false(cells 模式)时 width/height 是列/行,要换算成像素目标。
                        if let ClientMessage::Sync {
                            width,
                            height,
                            pixels,
                            ..
                        } = &cm
                        {
                            if *pixels {
                                sync_width = *width;
                                sync_height = *height;
                            } else {
                                sync_width = (*width as u32
                                    * crate::ws::SCREEN_CHAR_WIDTH
                                    + 2 * crate::ws::SCREEN_PADDING) as u16;
                                sync_height = (*height as u32
                                    * crate::ws::SCREEN_CHAR_HEIGHT
                                    + 2 * crate::ws::SCREEN_PADDING) as u16;
                            }
                        }
                        if let Err(e) = cli_tx.send(cm).await {
                            log::error!("[mqtt] cli_tx send failed: {e}");
                            break;
                        }
                    }
                }
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    // 每次(重)连接建立后(重新)订阅入站 topic。重连后 broker 侧订阅
                    // 可能已丢,不重订阅就会出现「能发收不到」。幂等:重复订阅 broker 去重。
                    for (name, qos) in INBOUND_TOPICS {
                        if let Err(e) = client
                            .subscribe(format!("{prefix}/{name}"), qos_from_u8(*qos))
                            .await
                        {
                            log::warn!("[mqtt] (re)subscribe {prefix}/{name} failed: {e}");
                        }
                    }
                    // 重连后补发 presence:断连时 LWT 已清空,不补发 ESP32 要等下次心跳(15s)
                    // 才知道实例又上线。用最新 title/state 重新生成 payload(带新 ts——不能
                    // 复用旧 payload,旧 ts 会被 ESP32 当心跳超时)。首次连接时还没有 presence,
                    // 跳过(等 ws 触发首次公告)。
                    if let Some((title, state)) = last_presence.clone() {
                        let payload = presence_payload(&prefix, &client_id, &title, state, image_format);
                        if let Err(e) = client
                            .publish(prefix.clone(), QoS::AtLeastOnce, true, payload)
                            .await
                        {
                            log::warn!("[mqtt] presence re-publish on reconnect failed: {e}");
                        }
                    }
                    log::info!("[mqtt] connected, (re)subscribed inbound topics under {prefix}/");
                }
                Ok(_) => {}
                Err(e) => {
                    // rumqttc EventLoop 自带自动重连;这里只记录并退避,WS/PTY 不受影响。
                    log::warn!("[mqtt] eventloop error (auto-reconnecting): {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            },
            msg = rx.recv() => match msg {
                Ok(ServerMessage::PtyOutput(bytes)) => {
                    // text 模式增量(pty_out):ws 仅在 text 模式发 PtyOutput。前面加 tag=0x01,
                    // 发到 screen_text(与全屏帧同 topic,靠首字节区分)。不 retained(屏 topic
                    // 一律不 retained,避免 pid 变更后老 topic 残留清不掉)。
                    let mut payload = Vec::with_capacity(1 + bytes.len());
                    payload.push(0x01);
                    payload.extend_from_slice(&bytes);
                    total_screen_bytes += payload.len() as u64;
                    log::debug!(
                        "[mqtt] screen_text delta {} bytes (total {:.2} MB) -> {screen_text_topic}",
                        payload.len(),
                        total_screen_bytes as f64 / (1024.0 * 1024.0)
                    );
                    if let Err(e) = client
                        .publish(&screen_text_topic, QoS::AtMostOnce, false, payload)
                        .await
                    {
                        log::warn!("[mqtt] publish screen_text delta failed: {e}");
                    }
                }
                Ok(ServerMessage::Screen(screen)) => {
                    // 按 image_format 决定渲染成 JPEG(发 {prefix}/screen)还是 ANSI 文本
                    // (发 {prefix}/screen_text)。**都不 retained**(屏 topic 一律不 retain,
                    // 避免 pid 变更后老 topic 的 retained 残留清不掉),QoS0。ESP32 重连靠 sync 拿首帧。
                    if image_format.is_text() {
                        // text 模式全屏基线:tag=0x00 + contents_formatted(可重放的 ANSI 流)。
                        // 不 retained,之后靠 delta(pty_out)增量;ESP32 连上发 sync 触发本帧。
                        let text = render_screen_to_text(screen.as_ref());
                        let mut payload = Vec::with_capacity(1 + text.len());
                        payload.push(0x00);
                        payload.extend_from_slice(text.as_bytes());
                        total_screen_bytes += payload.len() as u64;
                        log::debug!(
                            "[mqtt] screen_text full {} bytes (total {:.2} MB) -> {screen_text_topic}",
                            payload.len(),
                            total_screen_bytes as f64 / (1024.0 * 1024.0)
                        );
                        if let Err(e) = client
                            .publish(&screen_text_topic, QoS::AtMostOnce, false, payload)
                            .await
                        {
                            log::warn!("[mqtt] publish screen_text failed: {e}");
                        }
                    } else {
                        let sync_w = sync_width;
                        let sync_h = sync_height;
                        let target = (sync_w != 0 && sync_h != 0).then_some((sync_w, sync_h));
                        match render_screen_to_image(screen.as_ref(), image_format, target) {
                            Ok(mut img) if !img.is_empty() => {
                                // 图片末尾追加当前 scrollback offset(u32 大端=网络序,4 字节):IEND/EOI
                                // 之后解码器忽略,接收端读末 4 字节即知「这张图截自滚到第 N 行」
                                // (0=底部/最新)。offset 直接从 Screen 读——它自带 scrollback_offset 字段。
                                let offset = screen.as_ref().scrollback() as u32;
                                img.extend_from_slice(&offset.to_be_bytes());
                                total_screen_bytes += img.len() as u64;
                                log::debug!(
                                    "[mqtt] screen image {} bytes (trailer offset={offset}, total {:.2} MB) -> {screen_topic}",
                                    img.len(),
                                    total_screen_bytes as f64 / (1024.0 * 1024.0)
                                );
                                if let Err(e) = client
                                    .publish(&screen_topic, QoS::AtMostOnce, false, img)
                                    .await
                                {
                                    log::warn!("[mqtt] publish screen failed: {e}");
                                }
                            }
                            Ok(_) => {}
                            Err(e) => log::warn!("[mqtt] render screen failed: {e}"),
                        }
                    }
                }
                Ok(ServerMessage::Presence { title, state }) => {
                    // ws 主循环定期(心跳)+ 状态翻转时发来 → publish presence(retained)。
                    // 缓存 title/state,供重连后补发(见 ConnAck 分支)。
                    last_presence = Some((title.clone(), state));
                    let payload = presence_payload(&prefix, &client_id, &title, state, image_format);
                    if let Err(e) = client
                        .publish(prefix.clone(), QoS::AtLeastOnce, true, payload)
                        .await
                    {
                        log::warn!("[mqtt] presence publish failed: {e}");
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("[mqtt] broadcast lagged {n} messages");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    log::info!("[mqtt] broadcast closed, stop client");
                    break;
                }
            },
        }
    }

    // 故意**不发 MQTT DISCONNECT**:直接让连接随函数返回被 drop(socket 关闭),broker
    // 视为异常掉线 → **必发 LWT**(空 retained)清掉 presence。这样无论进程退出还是面板
    // Stop client,presence 都会被清,不残留(干净 DISCONNECT 的话 broker 不发 LWT,presence
    // 会留在老 pid 的 topic 上)。rumqttc 0.25.1 的 client/eventloop 没有 Drop impl,
    // drop 时不会自己补发 DISCONNECT,正好。
    log::info!("[mqtt] client stopped (connection dropped; broker fires LWT to clear presence)");
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
