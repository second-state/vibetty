//! 内置 rumqttd broker:`builtin_broker=true` 时,在 vibetty 进程内启动一个
//! rumqttd,监听 TCP(`port`)+ WebSocket(`builtin_ws_port`),匿名认证。
//!
//! vibetty 自身的 MQTT client(`mqtt.rs`)改连 127.0.0.1;ESP32 直接连本机 `port`。
//! `Broker::start()` 是阻塞的(内部 join 各 server 线程),所以用独立 OS 线程跑,
//! 不阻塞 tokio runtime。

use std::collections::HashMap;
use std::net::SocketAddr;

use rumqttd::{Config, ConnectionSettings, RouterConfig, ServerSettings};

use crate::config::MqttConfig;

/// 启动内置 rumqttd broker(后台 OS 线程)。返回后 broker 已在跑。
///
/// 这里返回的失败仅限构造配置 / 建线程阶段;broker 运行时错误走日志(它自己
/// 内部用 tracing,vibetty 没装 tracing subscriber 时这些日志会被丢弃,不影响功能)。
pub fn spawn_builtin(cfg: &MqttConfig) -> anyhow::Result<()> {
    let rcfg = build_config(cfg)?;
    std::thread::Builder::new()
        .name("rumqttd-broker".to_string())
        .spawn(move || {
            let mut broker = rumqttd::Broker::new(rcfg);
            if let Err(e) = broker.start() {
                log::error!("[broker] rumqttd exited: {e:?}");
            }
        })?;
    Ok(())
}

/// 构造 `rumqttd::Config`:一个 TCP(v4)listener + 一个 WS listener,均匿名。
fn build_config(cfg: &MqttConfig) -> anyhow::Result<Config> {
    let connections = ConnectionSettings {
        connection_timeout_ms: 60000,
        // screen PNG / pty_out 可能几十~几百 KB;rumqttd 默认 20480 会拒大包。
        // 和 vibetty client 的 set_max_packet_size 对齐(1MB)。
        max_payload_size: 1024 * 1024,
        max_inflight_count: 100,
        auth: None, // 匿名
        external_auth: None,
        dynamic_filters: false,
    };

    let tcp = ServerSettings {
        name: "tcp".to_string(),
        listen: format!("0.0.0.0:{}", cfg.builtin_port).parse::<SocketAddr>()?,
        tls: None,
        next_connection_delay_ms: 1,
        connections: connections.clone(),
    };
    let ws = ServerSettings {
        name: "ws".to_string(),
        listen: format!("0.0.0.0:{}", cfg.builtin_ws_port).parse::<SocketAddr>()?,
        tls: None,
        next_connection_delay_ms: 1,
        connections,
    };

    let mut v4 = HashMap::new();
    v4.insert("tcp".to_string(), tcp);
    let mut ws_map = HashMap::new();
    ws_map.insert("ws".to_string(), ws);

    Ok(Config {
        id: 0,
        router: RouterConfig {
            max_connections: 10010,
            max_outgoing_packet_count: 200,
            max_segment_size: 1024 * 1024,
            max_segment_count: 10000,
            custom_segment: None,
            initialized_filters: None,
            shared_subscriptions_strategy: Default::default(),
        },
        v4: Some(v4),
        v5: None,
        ws: Some(ws_map),
        cluster: None,
        console: None,
        bridge: None,
        prometheus: None,
        metrics: None,
    })
}
