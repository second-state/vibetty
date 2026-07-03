//! 内置 broker 冒烟测试:起一个 rumqttd(127.0.0.1:18830),用 rumqttc 客户端
//! subscribe + publish,验证 broker 监听 / MQTT 握手 / 消息转发都正常。
//!
//! 跑法(在 vibetty 目录):`cargo run --example builtin_smoke`

use std::collections::HashMap;
use std::time::Duration;

use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use rumqttd::{Config, ConnectionSettings, RouterConfig, ServerSettings};

const PORT: u16 = 18830; // 用冷门端口,避免和本机 mosquitto(1883)冲突

fn build_config() -> Config {
    let connections = ConnectionSettings {
        connection_timeout_ms: 60000,
        max_payload_size: 256 * 1024,
        max_inflight_count: 100,
        auth: None, // 匿名
        external_auth: None,
        dynamic_filters: false,
    };
    let mut v4 = HashMap::new();
    v4.insert(
        "tcp".to_string(),
        ServerSettings {
            name: "tcp".to_string(),
            listen: format!("127.0.0.1:{PORT}").parse().unwrap(),
            tls: None,
            next_connection_delay_ms: 1,
            connections,
        },
    );
    Config {
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
        ws: None,
        cluster: None,
        console: None,
        bridge: None,
        prometheus: None,
        metrics: None,
    }
}

#[tokio::main]
async fn main() {
    // Broker::start() 阻塞,放独立 OS 线程(和 broker.rs::spawn_builtin 一致)
    std::thread::Builder::new()
        .name("rumqttd-smoke".to_string())
        .spawn(move || {
            let mut broker = rumqttd::Broker::new(build_config());
            if let Err(e) = broker.start() {
                eprintln!("[smoke] broker exited: {e:?}");
            }
        })
        .unwrap();

    // 给 broker 一点时间监听就绪
    tokio::time::sleep(Duration::from_millis(800)).await;

    let mut opts = MqttOptions::new("smoke-client", "127.0.0.1", PORT);
    opts.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(opts, 10);

    client
        .subscribe("smoke/test", QoS::AtLeastOnce)
        .await
        .expect("subscribe");
    client
        .publish(
            "smoke/test",
            QoS::AtLeastOnce,
            false,
            b"hello-builtin-broker",
        )
        .await
        .expect("publish");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if tokio::time::Instant::now() > deadline {
            eprintln!("SMOKE FAIL: timed out waiting for published message");
            std::process::exit(1);
        }
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) if p.topic == "smoke/test" => {
                println!(
                    "SMOKE OK: builtin broker forwarded payload = {:?}",
                    String::from_utf8_lossy(&p.payload)
                );
                return;
            }
            Ok(_) => {}
            Err(e) => eprintln!("[smoke] poll error: {e}"),
        }
    }
}
