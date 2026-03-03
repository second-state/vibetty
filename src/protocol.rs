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

    /// 客户端选择
    #[serde(rename = "choice")]
    Choice {
        /// 选项索引（choices 数组的索引）
        index: i32,
    },

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
            ClientMessage::Choice { index } => {
                f.debug_struct("Choice").field("index", index).finish()
            }
            ClientMessage::ChangeDir(path) => {
                f.debug_tuple("ChangeDir").field(path).finish()
            }
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

    /// 屏幕显示文本
    #[serde(rename = "screen_text")]
    ScreenText(ScreenTextData),

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

    /// 提供选择项
    #[serde(rename = "choices")]
    Choices(ChoicesData),
}

/// 屏幕显示文本数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenTextData {
    /// 文本内容
    pub text: String,
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
}

/// 请求输入数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInputData {
    /// 提示语
    pub prompt: String,
}

/// 提供选择项数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoicesData {
    /// 工具调用 ID（用于识别是否是同一个 tool 请求）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// 标题/问题
    pub title: String,

    /// 选择项列表, 如果是空则表示选择 confirm/cancel
    pub options: Vec<String>,
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
}

// ========== 客户端消息构造 ==========

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

    /// 创建客户端选择消息
    pub fn choice(index: i32) -> Self {
        Self::Choice { index }
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

impl ServerMessage {
    /// 创建 PTY 输出消息
    pub fn pty_output(data: Vec<u8>) -> Self {
        Self::PtyOutput(data)
    }

    /// 创建屏幕文本消息
    pub fn screen_text(text: impl Into<String>) -> Self {
        Self::ScreenText(ScreenTextData { text: text.into() })
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
        })
    }

    /// 创建请求输入消息
    pub fn get_input(prompt: impl Into<String>) -> Self {
        Self::GetInput(GetInputData {
            prompt: prompt.into(),
        })
    }

    /// 创建提供选择项消息
    pub fn choices(title: impl Into<String>, options: Vec<String>) -> Self {
        Self::Choices(ChoicesData {
            id: None,
            title: title.into(),
            options,
        })
    }

    /// 创建提供选择项消息（带 ID）
    pub fn choices_with_id(id: impl Into<String>, title: impl Into<String>, options: Vec<String>) -> Self {
        Self::Choices(ChoicesData {
            id: Some(id.into()),
            title: title.into(),
            options,
        })
    }

    /// 创建 ASR 结果消息
    pub fn asr_result(text: impl Into<String>) -> Self {
        Self::AsrResult(text.into())
    }
}

// ========== MessagePack 序列化 ==========

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
    fn test_client_choice_json() {
        let msg = ClientMessage::choice(2);
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ClientMessage::from_json(&json).unwrap();
        match decoded {
            ClientMessage::Choice { index } => {
                assert_eq!(index, 2);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_client_choice_msgpack() {
        let msg = ClientMessage::choice(2);
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ClientMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ClientMessage::Choice { index } => {
                assert_eq!(index, 2);
            }
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
    fn test_server_screen_text_msgpack() {
        let msg = ServerMessage::screen_text("Hello, World!");
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::ScreenText(data) => {
                assert_eq!(data.text, "Hello, World!");
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
    fn test_server_choices_msgpack() {
        let msg = ServerMessage::choices(
            "请选择",
            vec![
                "选项A".to_string(),
                "选项B".to_string(),
                "选项C".to_string(),
            ],
        );
        let bytes = msg.to_msgpack().unwrap();
        let decoded = ServerMessage::from_msgpack(&bytes).unwrap();
        match decoded {
            ServerMessage::Choices(data) => {
                assert_eq!(data.title, "请选择");
                assert_eq!(data.options.len(), 3);
                assert_eq!(data.options[0], "选项A");
                assert_eq!(data.options[1], "选项B");
                assert_eq!(data.options[2], "选项C");
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
    fn test_server_screen_text_json() {
        let msg = ServerMessage::screen_text("Hello");
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ServerMessage::from_json(&json).unwrap();
        match decoded {
            ServerMessage::ScreenText(data) => {
                assert_eq!(data.text, "Hello");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_choices_json() {
        let msg = ServerMessage::choices("请选择", vec!["A".into(), "B".into()]);
        let json = msg.to_json().unwrap();
        println!("JSON: {}", json);
        let decoded = ServerMessage::from_json(&json).unwrap();
        match decoded {
            ServerMessage::Choices(data) => {
                assert_eq!(data.title, "请选择");
                assert_eq!(data.options, vec!["A", "B"]);
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
