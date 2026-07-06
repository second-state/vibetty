use clap::{Parser, Subcommand};

/// 可选的 MQTT 传输配置。`~/.vibetty/config.toml` 里没有 `[mqtt]` 段时为 None,
/// 表示完全不启用 MQTT(现有 WebSocket/HTTP 行为不变)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MqttConfig {
    /// 是否启用 MQTT 传输;设为 false 可保留配置但关闭(默认 true)
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Broker URL:`mqtt://[user:pass@]host[:port]`(明文)或 `mqtts://...`(TLS)。
    /// user/pass/TLS/端口 都从这个 URL 解析(scheme 决定 TLS)。
    /// `port` 字段只在内置 broker 模式下单独用。
    #[serde(default)]
    pub broker: String,
    /// 内置 broker 的 TCP 监听端口(默认 1883)。URL 没写端口时按协议默认(mqtt 1883 / mqtts 8883)。
    #[serde(default = "default_mqtt_port")]
    pub builtin_port: u16,
    /// QoS: 0 / 1 / 2,默认 1(AtLeastOnce,适合弱网)
    #[serde(default = "default_mqtt_qos")]
    pub qos: u8,
    /// keep-alive 秒数,默认 30
    #[serde(default = "default_keep_alive")]
    pub keep_alive_secs: u64,
    /// 是否在进程内启动内置 rumqttd broker(默认 false)。
    /// 为 true 时:vibetty 自带 broker,监听 port(TCP)+ builtin_ws_port(WS),
    /// 自身的 client 改连 127.0.0.1;ESP32 直接连本机 port。broker(user/pass/TLS)被忽略。
    /// 注意:匿名认证 + 监听 0.0.0.0,仅内网使用,勿暴露公网。
    #[serde(default)]
    pub builtin_broker: bool,
    /// 内置 broker 的 WebSocket 端口(默认 9001),仅 builtin_broker=true 时生效。
    #[serde(default = "default_ws_port")]
    pub builtin_ws_port: u16,
}

fn default_true() -> bool {
    true
}
fn default_mqtt_port() -> u16 {
    1883
}
fn default_mqtt_qos() -> u8 {
    1
}
fn default_keep_alive() -> u64 {
    30
}
fn default_ws_port() -> u16 {
    9001
}

#[derive(Parser, Debug)]
#[command(name = "vibetty")]
#[command(about = "WebSocket terminal server", long_about = None, version)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to config.toml (overrides default ~/.vibetty/config.toml)
    #[arg(long, value_name = "PATH", global = true)]
    pub config: Option<std::path::PathBuf>,

    /// Listen address (e.g., "0.0.0.0:3000")
    #[arg(short, long, default_value = "0.0.0.0:3000")]
    pub bind_addr: String,

    #[arg(short, long, default_value = "true")]
    pub auto_submit: bool,

    /// Command to execute on PTY start (e.g., -- bash -l)
    #[arg(last = true)]
    pub command_args: Vec<String>,

    /// Image format for screen rendering (png or jpeg)
    #[arg(short = 'f', long, default_value = "png", value_name = "FORMAT")]
    pub image_format: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Configure MQTT transport via TUI (writes ~/.vibetty/config.toml)
    Setup,
}

impl Cli {
    pub fn run_args(&self) -> RunArgs {
        RunArgs {
            bind_addr: self.bind_addr.clone(),
            auto_submit: self.auto_submit,
            command: self.command_args.clone(),
            image_format: self.image_format.clone(),
            config: self.config.clone(),
        }
    }
}

/// Run-mode args.
pub struct RunArgs {
    pub bind_addr: String,
    pub auto_submit: bool,
    pub command: Vec<String>,
    pub image_format: String,
    pub config: Option<std::path::PathBuf>,
}

impl RunArgs {
    pub fn image_format(&self) -> crate::protocol::ImageFormat {
        match self.image_format.to_lowercase().as_str() {
            "jpeg" | "jpg" => crate::protocol::ImageFormat::Jpeg,
            _ => crate::protocol::ImageFormat::Png,
        }
    }

    /// 读取可选的 `[mqtt]` 配置。
    /// 优先用 `--config` 指定的路径,否则回退 `~/.vibetty/config.toml`。
    /// 无配置文件 / 无 `[mqtt]` 段 → None(不启用 MQTT,现有 WebSocket/HTTP 路径不变)。
    pub fn mqtt_config(&self) -> Option<MqttConfig> {
        #[derive(serde::Deserialize)]
        struct MqttSection {
            #[serde(default)]
            mqtt: Option<MqttConfig>,
        }
        let path = match self.config.as_ref() {
            Some(p) => p.clone(),
            None => dirs::home_dir()?.join(".vibetty").join("config.toml"),
        };
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str::<MqttSection>(&content).ok()?.mqtt
    }
}
