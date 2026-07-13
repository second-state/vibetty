use std::fmt::Debug;

use serde::{Deserialize, Serialize};

use crate::terminal::agent::AgentState;

// ========== 客户端 -> 服务器 ==========

/// 客户端发送的消息
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    /// Sync:客户端声明自己显示区的【像素】尺寸 `width`/`height`,
    /// 服务端按 char cell 尺寸换算成列/行后 resize PTY,并回送整张屏幕
    #[serde(rename = "sync")]
    Sync { width: u16, height: u16 },

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

impl Debug for ClientMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientMessage::Sync { width, height } => f
                .debug_struct("Sync")
                .field("width", width)
                .field("height", height)
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
    /// PTY 输出（终端输出显示）。当前停发 broadcast(mqtt 停发 pty_out、WS 前端已删,
    /// 无人消费),保留变体供将来恢复。`#[allow(dead_code)]` 抑制「无人构造」警告。
    #[serde(rename = "pty_out")]
    #[allow(dead_code)]
    PtyOutput(Vec<u8>),

    /// 整张终端屏幕(MQTT 出站据此渲染成图片;不走 serde 序列化)
    #[serde(skip)]
    Screen(std::sync::Arc<vt100::Screen>),

    /// presence 公告(含窗口 title + agent 工作状态),由 ws 主循环定期(心跳)及状态
    /// 翻转时发出。MQTT 收到后在本实例前缀 topic 发 retained presence。`#[serde(skip)]`
    /// —— 只走 MQTT,不走 WS 浏览器。
    #[serde(skip)]
    Presence { title: String, state: AgentState },
}

// ========== 辅助类型 ==========

/// screen 出图的 JPEG 质量档位。出图始终为 JPEG;High/Medium 彩色,Low 黑白(灰度)。
#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JpegQuality {
    High,
    Medium,
    Low,
}

impl JpegQuality {
    /// 对应的 HTTP MIME 类型(三档都是 JPEG)
    pub fn mime_type(&self) -> &'static str {
        "image/jpeg"
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
}
