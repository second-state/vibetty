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
    /// 内置 broker 的 TCP 监听端口(默认 1883)。URL 没写端口时按协议默认(mqtt 1883 / mqtts 8883)。
    #[serde(default = "default_mqtt_port")]
    pub builtin_port: u16,
}

impl MqttConfig {
    /// 返回一份用于**启动传输 client**(`mqtt::spawn`)的配置副本。
    ///
    /// broker URL 一律以 config 里的 `broker` 为准;**只有** `builtin_broker=true` 且 `broker` 为空时,
    /// 才默认填上本地内置 broker 地址(`mqtt://127.0.0.1:{builtin_port}`)。也就是说:即便内置 broker
    /// 开着,只要 config 里填了 `broker`,client 就连那个地址(不会强制改本地)。
    /// boot 自动起 + 运行期(重)spawn + 面板预填/比对 都复用这个,保证 URL 解析逻辑只有一处。
    pub fn for_client(&self) -> MqttConfig {
        let mut c = self.clone();
        if c.builtin_broker && c.broker.trim().is_empty() {
            c.broker = format!("mqtt://127.0.0.1:{}", c.builtin_port);
        }
        c
    }
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
#[command(about = "MQTT terminal server", long_about = None, version)]
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

    /// Screen output format: text (plain ANSI text stream, no image) or high/medium/low (JPEG quality tiers)
    #[arg(
        short = 'q',
        long = "quality",
        default_value = "text",
        value_name = "QUALITY"
    )]
    pub image_format: String,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Configure MQTT transport via TUI (writes ~/.vibetty/config.toml)
    Setup,
    /// Install/uninstall the built-in run-vibetty SKILL.md into an agent's user-level skills dir
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillAction {
    /// Write the bundled SKILL.md into ~/.claude/skills/run-vibetty/ and/or ~/.agents/skills/run-vibetty/
    Install {
        /// Target Claude Code (writes ~/.claude/skills/run-vibetty/SKILL.md)
        #[arg(long)]
        claude: bool,
        /// Target Codex USER scope (writes ~/.agents/skills/run-vibetty/SKILL.md)
        #[arg(long)]
        codex: bool,
    },
    /// Remove the SKILL.md (and the dir if it becomes empty) from the agent's skills dir
    Uninstall {
        /// Target Claude Code
        #[arg(long)]
        claude: bool,
        /// Target Codex
        #[arg(long)]
        codex: bool,
    },
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
    pub fn image_format(&self) -> crate::protocol::OutputFormat {
        match self.image_format.to_lowercase().as_str() {
            "medium" | "mid" | "m" => crate::protocol::OutputFormat::Medium,
            "low" | "l" => crate::protocol::OutputFormat::Low,
            "text" | "txt" | "t" => crate::protocol::OutputFormat::Text,
            _ => crate::protocol::OutputFormat::High,
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
