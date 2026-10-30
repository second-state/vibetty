use clap::{Parser, Subcommand};

/// 可选的 MQTT 传输配置。`~/.vibetty/config.toml` 里没有 `[mqtt]` 段时为 None,
/// 表示完全不启用 MQTT。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MqttConfig {
    /// Broker URL:`mqtt://[user:pass@]host[:port]`(明文)或 `mqtts://...`(TLS)。
    /// user/pass/TLS/端口 都从这个 URL 解析(scheme 决定 TLS)。
    /// host 指向回环地址时,vibetty 隐式启动内置 broker,监听 URL 里的端口。
    #[serde(default)]
    pub broker: String,
    /// keep-alive 秒数,默认 30
    #[serde(default = "default_keep_alive")]
    pub keep_alive_secs: u64,
    /// 内置 broker 的 WebSocket 端口。**0 = 不开 WS listener**(只 TCP);未配置时默认 0。
    /// 之前默认 9001:非零端口固定尝试 bind,撞上被占就整个 broker 起不来。
    #[serde(default)]
    pub builtin_ws_port: u16,
}

impl MqttConfig {
    /// 返回一份用于**启动传输 client**(`mqtt::spawn`)的配置副本。
    ///
    /// broker URL 一律以 config 里的 `broker` 为准;**只有** 内置 broker 模式
    /// (broker 指向回环地址,见 [`Self::wants_builtin_broker`])且 `broker` 为空时,
    /// 才默认填上本地内置 broker 地址(`mqtt://127.0.0.1:1883`)。
    /// boot 自动起 + 运行期(重)spawn + 面板预填/比对 都复用这个,保证 URL 解析逻辑只有一处。
    pub fn for_client(&self) -> MqttConfig {
        let mut c = self.clone();
        if self.wants_builtin_broker() && c.broker.trim().is_empty() {
            c.broker = "mqtt://127.0.0.1:1883".to_string();
        }
        c
    }

    /// 是否要启动内置 broker:`broker` 指向回环地址
    /// (localhost / 127.x / ::1 / [::1])。监听端口取该 URL 的端口
    /// (没写则按协议默认),见 [`Self::builtin_port`]。
    pub fn wants_builtin_broker(&self) -> bool {
        self.broker_is_loopback()
    }

    /// `broker` 是否指向回环地址。解析不了(空串/格式错)按 false 处理。
    pub fn broker_is_loopback(&self) -> bool {
        let Some((_, host)) = split_broker_host_port(&self.broker) else {
            return false;
        };
        matches!(
            host.to_ascii_lowercase().as_str(),
            "localhost" | "::1" | "[::1]"
        ) || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    }

    /// 内置 broker 的 TCP 监听端口:取 broker URL 里写的端口;
    /// 没写则按协议默认(mqtt 1883 / mqtts 8883)。
    pub fn builtin_port(&self) -> u16 {
        split_broker_host_port(&self.broker)
            .map(|(port, _)| port)
            .unwrap_or(1883)
    }
}

/// 从 broker URL 解析出 (端口, 主机名)。解析失败返回 None(空串/无 scheme/无 host)。
fn split_broker_host_port(url: &str) -> Option<(u16, &str)> {
    let (_scheme, rest) = url.split_once("://")?;
    let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let hostport = rest.split('/').next()?;
    let default_port = if _scheme == "mqtts" { 8883 } else { 1883 };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(default_port)),
        None => (hostport, default_port),
    };
    if host.is_empty() {
        return None;
    }
    // IPv6 字面量([::1])去掉方括号,方便 is_loopback 判断。
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((port, host))
}

fn default_keep_alive() -> u64 {
    30
}

#[derive(Parser, Debug)]
#[command(name = "vibetty")]
#[command(about = "MQTT terminal server", long_about = None, version)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Config file path (default ~/.vibetty/config.toml)
    #[arg(long, value_name = "PATH", global = true)]
    pub config: Option<std::path::PathBuf>,

    #[arg(short, long, default_value = "true")]
    pub auto_submit: bool,

    /// Command to run in the PTY (e.g. -- bash -l)
    #[arg(last = true)]
    pub command_args: Vec<String>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Configure MQTT via TUI
    Setup,
    /// Install/uninstall the run-vibetty skill; with no subcommand, print the skill to stdout
    Skill {
        #[command(subcommand)]
        action: Option<SkillAction>,
    },
    /// Share a herdr agent terminal over MQTT
    Herdr {
        /// herdr agent target id (or VIBETTY_HERDR_TARGET env)
        target: Option<String>,
        #[arg(short = 'a', long = "auto-submit", default_value = "true")]
        auto_submit: bool,
    },
    /// herdr share action entrypoint (not for manual use)
    ShareHerdr,
    /// Update vibetty to the latest release (or a specific tag)
    Update {
        /// Release tag to install (default: latest stable, prereleases excluded)
        tag: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum SkillAction {
    /// Install the run-vibetty skill
    Install {
        /// Target Claude Code
        #[arg(long)]
        claude: bool,
        /// Target Codex
        #[arg(long)]
        codex: bool,
    },
    /// Uninstall the run-vibetty skill
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
            auto_submit: self.auto_submit,
            command: self.command_args.clone(),
            config: self.config.clone(),
        }
    }
}

/// Run-mode args.
pub struct RunArgs {
    pub auto_submit: bool,
    pub command: Vec<String>,
    pub config: Option<std::path::PathBuf>,
}

impl RunArgs {
    /// 读取可选的 `[mqtt]` 配置。
    /// 优先用 `--config` 指定的路径,否则回退 `~/.vibetty/config.toml`。
    /// 无配置文件 / 无 `[mqtt]` 段 → None(不启用 MQTT)。
    pub fn mqtt_config(&self) -> Option<MqttConfig> {
        read_mqtt_config(self.config.as_deref())
    }

    /// 读取可选的 `[ui] menu_key` 字符串(如 "ctrl+shift+p")。
    /// 未配置 → None(调用方用默认 `ctrl+shift+p`)。
    pub fn menu_key(&self) -> Option<String> {
        read_menu_key(self.config.as_deref())
    }
}

/// 读取可选的 `[mqtt]` 配置。
/// `config_path` 为 None 时回退 `~/.vibetty/config.toml`。
/// 无配置文件 / 无 `[mqtt]` 段 → None(不启用 MQTT)。
pub fn read_mqtt_config(config_path: Option<&std::path::Path>) -> Option<MqttConfig> {
    #[derive(serde::Deserialize)]
    struct MqttSection {
        #[serde(default)]
        mqtt: Option<MqttConfig>,
    }
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => dirs::home_dir()?.join(".vibetty").join("config.toml"),
    };
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str::<MqttSection>(&content).ok()?.mqtt
}

/// 读取可选的 `[ui] menu_key`(TUI 菜单弹窗快捷键,如 "ctrl+shift+p")。
/// 未配置 → None(调用方用默认值);解析交给 `ui::parse_keybinding`。
pub fn read_menu_key(config_path: Option<&std::path::Path>) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct UiSection {
        #[serde(default)]
        ui: Option<UiKeys>,
    }
    #[derive(serde::Deserialize)]
    struct UiKeys {
        #[serde(default)]
        menu_key: Option<String>,
    }
    let path = match config_path {
        Some(p) => p.to_path_buf(),
        None => dirs::home_dir()?.join(".vibetty").join("config.toml"),
    };
    let content = std::fs::read_to_string(&path).ok()?;
    toml::from_str::<UiSection>(&content).ok()?.ui?.menu_key
}
