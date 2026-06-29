use clap::{Parser, Subcommand};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WhisperASRConfig {
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "platform")]
pub enum AsrConfig {
    Whisper(WhisperASRConfig),
    /// vosk is a small local ASR engine. And it can run in browser.
    /// This option uses WebSocket to perform speech recognition in the browser via Vosk,
    /// sending results to the server. This avoids complex configuration and installation,
    /// enabling quick deployment and testing.
    WebVosk,
}

/// 可选的 MQTT 传输配置。`~/.vibetty/config.toml` 里没有 `[mqtt]` 段时为 None,
/// 表示完全不启用 MQTT(现有 WebSocket/HTTP 行为不变)。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MqttConfig {
    /// Broker 主机名/IP,例如 "broker.emqx.io" 或 "192.168.1.10"
    pub host: String,
    /// Broker 端口;1883=明文,8883=TLS
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    /// MQTT client id(broker 内需唯一)。留空则用 `vibetty-{pid}`
    #[serde(default)]
    pub client_id: String,
    /// 是否启用 TLS;留空则当 port==8883 时自动开启
    #[serde(default)]
    pub use_tls: Option<bool>,
    /// 用户名(broker 要求鉴权时填)
    #[serde(default)]
    pub username: Option<String>,
    /// 密码
    #[serde(default)]
    pub password: Option<String>,
    /// Topic 前缀,所有 topic 都在此前缀下。建议每台设备/会话不同
    #[serde(default = "default_topic_prefix")]
    pub topic_prefix: String,
    /// QoS: 0 / 1 / 2,默认 1(AtLeastOnce,适合弱网)
    #[serde(default = "default_mqtt_qos")]
    pub qos: u8,
    /// keep-alive 秒数,默认 30
    #[serde(default = "default_keep_alive")]
    pub keep_alive_secs: u64,
}

fn default_mqtt_port() -> u16 {
    1883
}
fn default_topic_prefix() -> String {
    "vibetty".to_string()
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

    /// ASR config file path (JSON format)
    #[arg(short = 'c', long)]
    pub asr_config_path: Option<String>,

    /// Command to execute on PTY start (e.g., -- bash -l)
    #[arg(last = true)]
    pub command_args: Vec<String>,

    /// Image format for screen rendering (png or jpeg)
    #[arg(short = 'f', long, default_value = "jpeg", value_name = "FORMAT")]
    pub image_format: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Setup ASR configuration via TUI
    Setup,
}

impl Cli {
    pub fn run_args(&self) -> RunArgs {
        RunArgs {
            bind_addr: self.bind_addr.clone(),
            auto_submit: self.auto_submit,
            asr_config_path: self.asr_config_path.clone(),
            command: self.command_args.clone(),
            image_format: self.image_format.clone(),
        }
    }
}

/// Separate struct for run-mode args, used by asr_config() logic.
pub struct RunArgs {
    pub bind_addr: String,
    pub auto_submit: bool,
    pub asr_config_path: Option<String>,
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

    pub fn asr_config(&self) -> AsrConfig {
        // 如果指定了配置文件，从文件读取
        if let Some(path) = &self.asr_config_path {
            if let Ok(content) = std::fs::read_to_string(path)
                && let Ok(config) = toml::from_str::<AsrConfig>(&content)
            {
                return config;
            }
            log::warn!(
                "Failed to parse ASR config from {}, falling back to env",
                path
            );
        } else if let Some(home) = dirs::home_dir() {
            let default_config = home.join(".vibetty").join("config.toml");
            if default_config.exists() {
                if let Ok(content) = std::fs::read_to_string(&default_config)
                    && let Ok(config) = toml::from_str::<AsrConfig>(&content)
                {
                    return config;
                }
                log::warn!(
                    "Failed to parse ASR config from {}, falling back to env",
                    default_config.display()
                );
            }
        }

        if std::env::var("VIBECODE_ASR_PLATFORM").unwrap_or_else(|_| "whisper".to_string())
            == "web_vosk"
        {
            return AsrConfig::WebVosk;
        }

        // 否则从环境变量读取
        AsrConfig::Whisper(WhisperASRConfig {
            url: std::env::var("VIBECODE_ASR_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1/audio/transcriptions".to_string()),
            api_key: std::env::var("VIBECODE_ASR_API_KEY").unwrap_or_default(),
            lang: std::env::var("VIBECODE_ASR_LANG").unwrap_or_else(|_| "".to_string()),
            model: std::env::var("VIBECODE_ASR_MODEL").unwrap_or_else(|_| "whisper-1".to_string()),
            prompt: std::env::var("VIBECODE_ASR_PROMPT").unwrap_or_default(),
        })
    }

    /// 读取可选的 `[mqtt]` 配置。无配置文件 / 无 `[mqtt]` 段 → None(不启用 MQTT,
    /// 现有 WebSocket/HTTP 路径完全不变)。与 `asr_config()` 用同一份 toml 文件。
    pub fn mqtt_config(&self) -> Option<MqttConfig> {
        #[derive(serde::Deserialize)]
        struct MqttSection {
            #[serde(default)]
            mqtt: Option<MqttConfig>,
        }
        let read = |path: &str| -> Option<MqttConfig> {
            let content = std::fs::read_to_string(path).ok()?;
            toml::from_str::<MqttSection>(&content).ok()?.mqtt
        };
        if let Some(path) = &self.asr_config_path {
            return read(path);
        }
        if let Some(home) = dirs::home_dir() {
            let p = home.join(".vibetty").join("config.toml");
            if p.exists() {
                return read(&p.to_string_lossy());
            }
        }
        None
    }
}
