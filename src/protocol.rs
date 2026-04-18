use std::fmt::Debug;

use serde::{Deserialize, Serialize};

// ========== 客户端 -> 服务器 ==========

/// 客户端发送的消息
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ClientMessage {
    /// Sync
    #[serde(rename = "sync")]
    Sync,

    /// PTY 输入（键盘输入发送到终端）
    #[serde(rename = "pty_in")]
    PtyInput(Vec<u8>),

    /// 开始语音输入
    #[serde(rename = "voice_input_start")]
    VoiceInputStart(VoiceInputStart),

    /// 语音数据块
    #[serde(rename = "voice_input_chunk")]
    VoiceInputChunk(Vec<u8>),

    /// 结束语音输入
    #[serde(rename = "voice_input_end")]
    VoiceInputEnd(VoiceInputEnd),

    /// 请求输入（文本输入）
    #[serde(rename = "input_text")]
    Input(String),

    /// 切换工作目录
    #[serde(rename = "change_dir")]
    ChangeDir(String),
}

impl Debug for ClientMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientMessage::Sync => f.debug_tuple("Sync").finish(),
            ClientMessage::PtyInput(data) => f
                .debug_tuple("PtyInput")
                .field(&format!("[{} bytes]", data.len()))
                .finish(),
            ClientMessage::VoiceInputStart(data) => {
                f.debug_tuple("VoiceInputStart").field(data).finish()
            }
            ClientMessage::VoiceInputChunk(data) => f
                .debug_tuple("VoiceInputChunk")
                .field(&format!("[{} bytes]", data.len()))
                .finish(),
            ClientMessage::VoiceInputEnd(_) => f.debug_tuple("VoiceInputEnd").finish(),
            ClientMessage::Input(text) => f.debug_tuple("Input").field(text).finish(),
            ClientMessage::ChangeDir(path) => f.debug_tuple("ChangeDir").field(path).finish(),
        }
    }
}

/// 语音输入开始数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInputStart {
    /// 采样率
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// 语音输入结束数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInputEnd {}

// ========== 服务器 -> 客户端 ==========

/// 服务器发送的消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ServerMessage {
    /// PTY 输出（终端输出显示）
    #[serde(rename = "pty_out")]
    PtyOutput(Vec<u8>),

    /// 屏幕显示图片
    #[serde(rename = "screen_image")]
    ScreenImage(ScreenImageData),

    /// 通知消息
    #[serde(rename = "notification")]
    Notification(NotificationData),

    /// 请求输入
    #[serde(rename = "get_input")]
    GetInput(GetInputData),

    /// ASR 结果
    #[serde(rename = "asr_result")]
    AsrResult(String),

    /// 设置状态栏
    #[serde(rename = "status")]
    Status(String),
}

/// 屏幕显示图片数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenImageData {
    /// 图片数据（原始字节）
    pub data: Vec<u8>,

    /// 图片格式
    pub format: ImageFormat,
}

/// 通知消息数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationData {
    /// 通知级别
    pub level: NotificationLevel,

    /// 通知内容
    pub message: String,

    /// 标题（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// 自定义 RGB 颜色（仅当 level 为 Custom 时生效）
    /// BE BIG-ENDIAN: 0xRRGGBB
    #[serde(default)]
    pub color: u32,
}

/// 请求输入数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInputData {
    /// 提示语
    pub prompt: String,
}

// ========== 辅助类型 ==========

/// 图片格式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
}

/// 通知级别
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    Info,
    Success,
    Warning,
    Error,
    /// 自定义颜色（需要配合 color 字段使用）
    Custom,
}

// ========== 客户端消息构造 ==========

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

    /// 创建语音输入开始消息
    pub fn voice_input_start(sample_rate: Option<u32>) -> Self {
        Self::VoiceInputStart(VoiceInputStart { sample_rate })
    }

    /// 创建语音数据块消息
    pub fn voice_input_chunk(data: Vec<u8>) -> Self {
        Self::VoiceInputChunk(data)
    }

    /// 创建语音输入结束消息
    pub fn voice_input_end() -> Self {
        Self::VoiceInputEnd(VoiceInputEnd {})
    }

    /// 创建文本输入消息
    pub fn input(text: impl Into<String>) -> Self {
        Self::Input(text.into())
    }

    /// 创建切换目录消息
    pub fn change_dir(path: impl Into<String>) -> Self {
        Self::ChangeDir(path.into())
    }
}

// ========== 服务器消息构造 ==========

#[allow(dead_code)]
impl ServerMessage {
    /// 创建 PTY 输出消息
    pub fn pty_output(data: Vec<u8>) -> Self {
        Self::PtyOutput(data)
    }

    /// 创建屏幕图片消息
    pub fn screen_image(data: Vec<u8>, format: ImageFormat) -> Self {
        Self::ScreenImage(ScreenImageData { data, format })
    }

    /// 创建通知消息
    pub fn notification(level: NotificationLevel, message: impl Into<String>) -> Self {
        Self::Notification(NotificationData {
            level,
            message: message.into(),
            title: None,
            color: 0,
        })
    }

    /// 创建自定义颜色的通知消息
    pub fn coustom_notification(
        message: impl Into<String>,
        title: Option<String>,
        color: u32,
    ) -> Self {
        Self::Notification(NotificationData {
            level: NotificationLevel::Custom,
            message: message.into(),
            title,
            color,
        })
    }

    /// 创建请求输入消息
    pub fn get_input(prompt: impl Into<String>) -> Self {
        Self::GetInput(GetInputData {
            prompt: prompt.into(),
        })
    }

    /// 创建 ASR 结果消息
    pub fn asr_result(text: impl Into<String>) -> Self {
        Self::AsrResult(text.into())
    }

    /// 创建状态栏消息
    pub fn status(text: impl Into<String>) -> Self {
        Self::Status(text.into())
    }
}

// ========== MessagePack 序列化 ==========

#[allow(dead_code)]
impl ClientMessage {
    /// 序列化为 MessagePack 字节
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(self)
    }

    /// 从 MessagePack 字节反序列化
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }

    /// 序列化为 JSON 字符串（备用）
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 JSON 字符串反序列化（备用）
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[allow(dead_code)]
impl ServerMessage {
    /// 序列化为 MessagePack 字节
    pub fn to_msgpack(&self) -> Result<Vec<u8>, rmp_serde::encode::Error> {
        rmp_serde::to_vec_named(self)
    }

    /// 从 MessagePack 字节反序列化
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }

    /// 序列化为 JSON 字符串（备用）
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 从 JSON 字符串反序列化（备用）
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_pty_input_msgpack() {
        let msg = ClientMessage::pty_input_str("hello");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::PtyInput(data) => {
                assert_eq!(String::from_utf8_lossy(&data), "hello");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_voice_input_start_msgpack() {
        let msg = ClientMessage::voice_input_start(Some(16000));
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::VoiceInputStart(data) => {
                assert_eq!(data.sample_rate, Some(16000));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_voice_input_chunk_msgpack() {
        let msg = ClientMessage::voice_input_chunk(vec![1, 2, 3, 4, 5]);
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::VoiceInputChunk(data) => {
                assert_eq!(data, vec![1, 2, 3, 4, 5]);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_voice_input_end_msgpack() {
        let msg = ClientMessage::voice_input_end();
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::VoiceInputEnd(_) => {}
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_input_msgpack() {
        let msg = ClientMessage::input("Hello, world!");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::Input(text) => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_notification_msgpack() {
        let msg = ServerMessage::notification(NotificationLevel::Info, "Test message");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::Notification(data) => {
                assert_eq!(data.message, "Test message");
                assert!(matches!(data.level, NotificationLevel::Info));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_get_input_msgpack() {
        let msg = ServerMessage::get_input("请说话");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::GetInput(data) => {
                assert_eq!(data.prompt, "请说话");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_status_msgpack() {
        let msg = ServerMessage::status("Connected");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::Status(text) => {
                assert_eq!(text, "Connected");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_asr_result_msgpack() {
        let msg = ServerMessage::asr_result("你好世界");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::AsrResult(text) => {
                assert_eq!(text, "你好世界");
            }
            _ => panic!("Wrong message type"),
        }
    }

    // ========== JSON 序列化测试 ==========

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
    fn test_client_voice_input_start_json() {
        let msg = ClientMessage::voice_input_start(Some(16000));
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::VoiceInputStart(data) => {
                assert_eq!(data.sample_rate, Some(16000));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_voice_input_chunk_json() {
        let msg = ClientMessage::voice_input_chunk(vec![1, 2, 3]);
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::VoiceInputChunk(data) => {
                assert_eq!(data, vec![1, 2, 3]);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_input_json() {
        let msg = ClientMessage::input("测试文本");
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::Input(text) => {
                assert_eq!(text, "测试文本");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_status_json() {
        let msg = ServerMessage::status("Ready");
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ServerMessage::from_json(&json).unwrap();
        match decoded {
            ServerMessage::Status(text) => {
                assert_eq!(text, "Ready");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_asr_result_json() {
        let msg = ServerMessage::asr_result("识别结果测试");
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ServerMessage::from_json(&json).unwrap();
        match decoded {
            ServerMessage::AsrResult(text) => {
                assert_eq!(text, "识别结果测试");
            }
            _ => panic!("Wrong message type"),
        }
    }
}
