use std::fmt::Debug;

use serde::{Deserialize, Serialize};

use crate::terminal::agent::AgentState;

// ========== 客户端 -> 服务器 ==========

/// 客户端发送的消息
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    /// Sync:客户端声明自己显示区尺寸 `width`/`height`。`pixels=true`(默认)时是【像素】,
    /// 服务端按 char cell 尺寸换算成列/行;`pixels=false` 时 width/height 已是字符列/行,直接用。
    /// 换算后 resize PTY 并回送整张屏幕。旧客户端不带 `pixels` → 默认 true(像素,向后兼容)。
    ///
    /// `close=true`:暂停服务端【自主】推送屏幕(PTY 输出触发的 screen/screen_text);`close=false`
    /// (默认):恢复。客户端(如 ESP32)不看时关掉省流量;sync 响应与 scroll 这类客户端主动请求
    /// 的回送不受影响。旧客户端不带 `close` → 默认 false(照常推送,向后兼容)。
    #[serde(rename = "sync")]
    Sync {
        width: u16,
        height: u16,
        #[serde(default = "default_sync_pixels")]
        pixels: bool,
        #[serde(default)]
        close: bool,
    },

    /// PTY 输入（键盘输入发送到终端）
    #[serde(rename = "pty_in")]
    PtyInput(Vec<u8>),

    /// 请求输入（文本输入）
    #[serde(rename = "input_text")]
    Input(String),

    /// 向上滚动;`rows` 缺省/0 = 滚一整页(= 终端可见行数)
    #[serde(rename = "scroll_up")]
    ScrollUp {
        #[serde(default)]
        rows: u16,
    },

    /// 向下滚动;同 `ScrollUp`
    #[serde(rename = "scroll_down")]
    ScrollDown {
        #[serde(default)]
        rows: u16,
    },
}

/// `Sync.pixels` 的 serde 默认值:true(像素)。旧客户端不带该字段时按像素处理,向后兼容。
fn default_sync_pixels() -> bool {
    true
}

impl Debug for ClientMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientMessage::Sync {
                width,
                height,
                pixels,
                close,
            } => f
                .debug_struct("Sync")
                .field("width", width)
                .field("height", height)
                .field("pixels", pixels)
                .field("close", close)
                .finish(),
            ClientMessage::PtyInput(data) => f
                .debug_tuple("PtyInput")
                .field(&format!("[{} bytes]", data.len()))
                .finish(),
            ClientMessage::Input(text) => f.debug_tuple("Input").field(text).finish(),
            ClientMessage::ScrollUp { rows } => f.debug_tuple("ScrollUp").field(rows).finish(),
            ClientMessage::ScrollDown { rows } => f.debug_tuple("ScrollDown").field(rows).finish(),
        }
    }
}

// ========== 服务器 -> 客户端 ==========

/// 服务器发送的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMessage {
    /// PTY 输出（终端输出显示）。原始 PTY 字节,ws 主循环每收到一段 PTY 输出就广播,
    /// MQTT 订阅后以 raw 二进制 publish 到 {prefix}/pty_out(QoS0,不 retained)。
    #[serde(rename = "pty_out")]
    PtyOutput(Vec<u8>),

    /// 整张终端屏幕(MQTT 出站据此渲染:JPEG 或 ANSI 文本,由 image_format 决定;不走 serde 序列化)
    #[serde(skip)]
    Screen(std::sync::Arc<vt100::Screen>),

    /// presence 公告(含窗口 title + agent 工作状态),由 ws 主循环定期(心跳)及状态
    /// 翻转时发出。MQTT 收到后在本实例前缀 topic 发 retained presence。`#[serde(skip)]`
    /// —— 只走 MQTT,不走 WS 浏览器。
    #[serde(skip)]
    Presence { title: String, state: AgentState },
}

// ========== 辅助类型 ==========

/// screen 出站格式档位。High/Medium 彩色 JPEG,Low 黑白(灰度)JPEG,Text 纯文本(不发图)。
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    High,
    Medium,
    Low,
    Text,
}

impl OutputFormat {
    /// 是否为文本模式(非图片)。
    pub fn is_text(&self) -> bool {
        matches!(self, OutputFormat::Text)
    }

    /// 字符串名(与 serde 序列化一致):`high`/`medium`/`low`/`text`。用于 presence 公告等。
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::High => "high",
            OutputFormat::Medium => "medium",
            OutputFormat::Low => "low",
            OutputFormat::Text => "text",
        }
    }

    /// 对应的 HTTP MIME 类型:High/Medium/Low 为 JPEG,Text 为纯文本。
    pub fn mime_type(&self) -> &'static str {
        match self {
            OutputFormat::Text => "text/plain; charset=utf-8",
            _ => "image/jpeg",
        }
    }
}

// ========== 客户端消息构造 / JSON ==========

#[allow(dead_code)]
impl ClientMessage {
    /// 创建 PTY 输入消息
    pub fn pty_input(data: Vec<u8>) -> Self {
        Self::PtyInput(data)
    }

    /// 创建 PTY 输入消息（从字符串）
    pub fn pty_input_str(s: &str) -> Self {
        Self::pty_input(s.as_bytes().to_vec())
    }

    /// 创建文本输入消息
    pub fn input(text: impl Into<String>) -> Self {
        Self::Input(text.into())
    }

    /// 序列化为 JSON 字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 JSON 字符串反序列化
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_pty_input_json() {
        let msg = ClientMessage::pty_input_str("hello");
        let json = msg.to_json().unwrap();
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::PtyInput(data) => {
                assert_eq!(String::from_utf8_lossy(&data), "hello");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_input_json() {
        let msg = ClientMessage::input("测试文本");
        let json = msg.to_json().unwrap();
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::Input(text) => {
                assert_eq!(text, "测试文本");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_sync_pixels_roundtrip() {
        // pixels=false + close=true 序列化/反序列化往返。
        let msg = ClientMessage::Sync {
            width: 320,
            height: 240,
            pixels: false,
            close: true,
        };
        let json = msg.to_json().unwrap();
        match ClientMessage::from_json(&json).unwrap() {
            ClientMessage::Sync {
                width,
                height,
                pixels,
                close,
            } => {
                assert_eq!((width, height), (320, 240));
                assert!(!pixels);
                assert!(close);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_sync_legacy_defaults_to_pixels() {
        // 老客户端不带 `pixels` / `close` 字段 → pixels 默认 true、close 默认 false,向后兼容。
        let legacy = r#"{"type":"sync","data":{"width":100,"height":50}}"#;
        match ClientMessage::from_json(legacy).unwrap() {
            ClientMessage::Sync {
                width,
                height,
                pixels,
                close,
            } => {
                assert_eq!((width, height), (100, 50));
                assert!(pixels, "legacy sync without `pixels` must default to true");
                assert!(!close, "legacy sync without `close` must default to false");
            }
            _ => panic!("Wrong message type"),
        }
    }
}
