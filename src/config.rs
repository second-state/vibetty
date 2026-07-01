use clap::{Parser, Subcommand};

/// 可选的 MQTT 传输配置。`~/.vibetty/config.toml` 里没有 `[mqtt]` 段时为 None,
/// 表示完全不启用 MQTT(现有 WebSocket/HTTP 行为不变)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MqttConfig {
    /// 是否启用 MQTT 传输;设为 false 可保留配置但关闭(默认 true)
    #[serde(default = "default_true")]
    pub enable: bool,
    /// Broker 主机名/IP,例如 "broker.emqx.io" 或 "192.168.1.10"
    pub host: String,
    /// Broker 端口;1883=明文,8883=TLS
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    /// MQTT client id(broker 内需唯一)。留空则用 `vibetty-{pid}`
    #[serde(default)]
    pub client_id: String,
    /// 是否启用 TLS;留空则当 port==8883 时自动开启
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_tls: Option<bool>,
    /// 用户名(broker 要求鉴权时填)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// 密码
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// QoS: 0 / 1 / 2,默认 1(AtLeastOnce,适合弱网)
    #[serde(default = "default_mqtt_qos")]
    pub qos: u8,
    /// keep-alive 秒数,默认 30
    #[serde(default = "default_keep_alive")]
    pub keep_alive_secs: u64,
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

impl MqttConfig {
    /// 解析后的有效 TLS 设置:显式优先,否则 port==8883 自动开
    pub fn effective_use_tls(&self) -> bool {
        self.use_tls.unwrap_or(self.port == 8883)
    }
    /// client_id 为空时兜底为 `vibetty-{pid}`
    pub fn effective_client_id(&self) -> String {
        if self.client_id.is_empty() {
            format!("vibetty-{}", std::process::id())
        } else {
            self.client_id.clone()
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "vibetty")]
#[command(about = "WebSocket terminal server", long_about = None, version)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Listen address (e.g., "0.0.0.0:3000")
    #[arg(short, long, default_value = "0.0.0.0:3000")]
    pub bind_addr: String,

    #[arg(short, long, default_value = "true")]
    pub auto_submit: bool,

    /// Command to execute on PTY start (e.g., -- bash -l)
    #[arg(last = true)]
    pub command_args: Vec<String>,

    /// Image format for screen rendering (png or jpeg)
    #[arg(short = 'f', long, default_value = "jpeg", value_name = "FORMAT")]
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
        }
    }
}

/// Run-mode args.
pub struct RunArgs {
    pub bind_addr: String,
    pub auto_submit: bool,
    pub command: Vec<String>,
    pub image_format: String,
}

impl RunArgs {
    pub fn image_format(&self) -> crate::protocol::ImageFormat {
        match self.image_format.to_lowercase().as_str() {
            "jpeg" | "jpg" => crate::protocol::ImageFormat::Jpeg,
            _ => crate::protocol::ImageFormat::Png,
        }
    }

    /// 读取可选的 `[mqtt]` 配置,固定从 `~/.vibetty/config.toml` 读取。
    /// 无配置文件 / 无 `[mqtt]` 段 → None(不启用 MQTT,现有 WebSocket/HTTP 路径不变)。
    pub fn mqtt_config(&self) -> Option<MqttConfig> {
        #[derive(serde::Deserialize)]
        struct MqttSection {
            #[serde(default)]
            mqtt: Option<MqttConfig>,
        }
        let home = dirs::home_dir()?;
        let path = home.join(".vibetty").join("config.toml");
        let content = std::fs::read_to_string(&path).ok()?;
        toml::from_str::<MqttSection>(&content).ok()?.mqtt
    }
}
