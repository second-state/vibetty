use clap::Parser;

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

#[derive(Parser, Debug)]
#[command(name = "vibetty")]
#[command(about = "WebSocket terminal server", long_about = None)]
pub struct Args {
    /// Listen address (e.g., "0.0.0.0:3000")
    #[arg(short, long, default_value = "0.0.0.0:3000")]
    pub bind_addr: String,

    /// ASR config file path (JSON format)
    #[arg(short = 'c', long)]
    pub asr_config_path: Option<String>,

    /// Command to execute on PTY start (e.g., -- bash -l)
    #[arg(last = true)]
    pub command: Vec<String>,
}

impl Args {
    pub fn asr_config(&self) -> AsrConfig {
        // 如果指定了配置文件，从文件读取
        if let Some(path) = &self.asr_config_path {
            if let Ok(content) = std::fs::read_to_string(path)
                && let Ok(config) = serde_json::from_str::<AsrConfig>(&content)
            {
                return config;
            }
            log::warn!(
                "Failed to parse ASR config from {}, falling back to env",
                path
            );
        }

        if std::env::var("ASR_PLATFORM").unwrap_or_else(|_| "whisper".to_string()) == "web_vosk" {
            return AsrConfig::WebVosk;
        }

        // 否则从环境变量读取
        AsrConfig::Whisper(WhisperASRConfig {
            url: std::env::var("ASR_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1/audio/transcriptions".to_string()),
            api_key: std::env::var("ASR_API_KEY").unwrap_or_default(),
            lang: std::env::var("ASR_LANG").unwrap_or_else(|_| "".to_string()),
            model: std::env::var("ASR_MODEL").unwrap_or_else(|_| "whisper-1".to_string()),
            prompt: std::env::var("ASR_PROMPT").unwrap_or_default(),
        })
    }
}
