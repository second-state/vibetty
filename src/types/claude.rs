#[derive(serde::Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClaudeCodeLog {
    #[serde(rename = "summary")]
    Summary(ClaudeCodeSummary),
    #[serde(rename = "file-history-snapshot")]
    Snapshot(ClaudeCodeHistorySnapshot),
    #[serde(rename = "user")]
    UserMessage(ClaudeCodeUserMessage),
    #[serde(rename = "assistant")]
    AssistantMessage(ClaudeCodeAssistantMessage),
    #[serde(rename = "system")]
    SystemMessage(ClaudeCodeSystemMessage),
}

impl ClaudeCodeLog {
    pub fn is_tool_request(&self) -> Option<(String, String, serde_json::Value)> {
        match self {
            ClaudeCodeLog::AssistantMessage(msg) => {
                for content in &msg.message.content {
                    if let ClaudeCodeMessageContent::ToolUse { name, input, id } = content {
                        return Some((id.clone(), name.clone(), input.clone()));
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn is_user_prompt(&self) -> Option<String> {
        match self {
            ClaudeCodeLog::UserMessage(msg) => match &msg.message {
                ClaudeCodeUserContent::Content { content, .. } => Some(content.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Returns (is_tool_result, is_error)
    pub fn is_tool_result(&self) -> (String, bool) {
        match self {
            ClaudeCodeLog::UserMessage(msg) => match &msg.message {
                ClaudeCodeUserContent::Complex { content, .. } => {
                    for item in content {
                        if let ComplexUserContent::ToolResult {
                            is_error,
                            tool_use_id,
                            ..
                        } = item
                        {
                            return (tool_use_id.clone(), *is_error);
                        }
                    }
                    (String::new(), false)
                }
                _ => (String::new(), false),
            },
            _ => (String::new(), false),
        }
    }

    pub fn is_output(&self) -> Option<(String, bool)> {
        match self {
            ClaudeCodeLog::AssistantMessage(msg) => {
                for content in &msg.message.content {
                    if let ClaudeCodeMessageContent::Text { text } = content {
                        return Some((text.clone(), false));
                    } else if let ClaudeCodeMessageContent::Thinking { thinking } = content {
                        return Some((thinking.clone(), true));
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn is_stop(&self) -> bool {
        match self {
            ClaudeCodeLog::SystemMessage(msg) => msg.stop_reason.is_some(),
            ClaudeCodeLog::UserMessage(ClaudeCodeUserMessage {
                message: ClaudeCodeUserContent::Complex { content, .. },
                ..
            }) => {
                if let ComplexUserContent::Text { text } = &content[0]
                    && text == "[Request interrupted by user for tool use]"
                {
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
}

// {"type":"summary","summary":"Gold Price Inquiry","leafUuid":"14800747-fd6e-4147-a60d-9b227aecf863"}
#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeSummary {
    #[serde(alias = "leafUuid")]
    pub leaf_uuid: String,
    pub summary: String,
}

// {"parentUuid":"2d399680-6d35-4150-bf3f-7f880e6bef02","isSidechain":false,"userType":"external","cwd":"/home/csh/my_workspace/speak_term","sessionId":"d284f444-9e56-4318-a472-bc18481b7793","version":"2.0.76","gitBranch":"main","type":"system","subtype":"stop_hook_summary","hookCount":1,"hookInfos":[{"command":"callback"}],"hookErrors":[],"preventedContinuation":false,"stopReason":"","hasOutput":false,"level":"suggestion","timestamp":"2026-02-03T18:13:21.125Z","uuid":"14800747-fd6e-4147-a60d-9b227aecf863","toolUseID":"6c1cb29a-49dc-40c9-b186-d42c6d58199d"}
#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeSystemMessage {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub uuid: String,
    #[serde(alias = "stopReason")]
    pub stop_reason: Option<String>,
}

// {"type":"file-history-snapshot","messageId":"442d98c4-af6a-429d-a0d7-7d725dd65618","snapshot":{"messageId":"442d98c4-af6a-429d-a0d7-7d725dd65618","trackedFileBackups":{},"timestamp":"2026-02-03T18:11:42.230Z"},"isSnapshotUpdate":false}
#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeHistorySnapshot {
    #[serde(alias = "messageId")]
    pub message_id: String,
    #[serde(alias = "isSnapshotUpdate")]
    pub is_snapshot_update: bool,
}

// {"parentUuid":null,"isSidechain":false,"userType":"external","cwd":"/home/csh/my_workspace/speak_term","sessionId":"d284f444-9e56-4318-a472-bc18481b7793","version":"2.0.76","gitBranch":"main","type":"user","message":{"role":"user","content":"帮我搜索一下今天黄金的价格"},"uuid":"442d98c4-af6a-429d-a0d7-7d725dd65618","timestamp":"2026-02-03T18:11:42.209Z","thinkingMetadata":{"level":"high","disabled":false,"triggers":[]},"todos":[]}
#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeUserMessage {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub uuid: String,
    pub timestamp: String,
    pub message: ClaudeCodeUserContent,
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
pub enum ClaudeCodeUserContent {
    Content {
        role: String,
        content: String,
    },
    Complex {
        role: String,
        content: [ComplexUserContent; 1],
    },
}

#[derive(serde::Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ComplexUserContent {
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(rename = "text")]
    Text { text: String },
}

// {"parentUuid":"442d98c4-af6a-429d-a0d7-7d725dd65618","isSidechain":false,"userType":"external","cwd":"/home/csh/my_workspace/speak_term","sessionId":"d284f444-9e56-4318-a472-bc18481b7793","version":"2.0.76","gitBranch":"main","message":{"id":"msg_202602040211443d9549301b2044b6","type":"message","role":"assistant","model":"glm-4.7","content":[{"type":"thinking","thinking":"用户想搜索今天黄金的价格。我可以使用 WebSearch 工具来获取这个信息。","signature":"0ccb904b724f48fab1d8d808"}],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":16216,"output_tokens":41,"cache_read_input_tokens":384,"server_tool_use":{"web_search_requests":0}}},"type":"assistant","uuid":"9f644493-751a-4d23-92ef-b469cad6dec7","timestamp":"2026-02-03T18:11:46.529Z"}
// {"parentUuid":"9f644493-751a-4d23-92ef-b469cad6dec7","isSidechain":false,"userType":"external","cwd":"/home/csh/my_workspace/speak_term","sessionId":"d284f444-9e56-4318-a472-bc18481b7793","version":"2.0.76","gitBranch":"main","message":{"id":"msg_202602040211443d9549301b2044b6","type":"message","role":"assistant","model":"glm-4.7","content":[{"type":"tool_use","id":"call_5bcf8991a9a046bd9780bedd","name":"WebSearch","input":{"query":"今日黄金价格 2026年2月4日"}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":16216,"output_tokens":41,"cache_read_input_tokens":384,"server_tool_use":{"web_search_requests":0}}},"type":"assistant","uuid":"df258c14-223d-4eb2-bd83-a90b7eea1d78","timestamp":"2026-02-03T18:11:46.539Z"}
#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeAssistantMessage {
    #[serde(alias = "sessionId")]
    pub session_id: String,
    pub uuid: String,
    pub timestamp: String,
    pub message: ClaudeCodeAssistantContent,
}

// {
//       "id": "msg_202602040211443d9549301b2044b6",
//       "type": "message",
//       "role": "assistant",
//       "model": "glm-4.7",
//       "content": [
//         {
//           "type": "thinking",
//           "thinking": "用户想搜索今天黄金的价格。我可以使用 WebSearch 工具来获取这个信息。",
//           "signature": "0ccb904b724f48fab1d8d808"
//         }
//       ],
//       "stop_reason": null,
//       "stop_sequence": null,
//       "usage": {
//         "input_tokens": 16216,
//         "output_tokens": 41,
//         "cache_read_input_tokens": 384,
//         "server_tool_use": {
//           "web_search_requests": 0
//         }
//       }
//     }

// {
//       "id": "msg_202602040211443d9549301b2044b6",
//       "type": "message",
//       "role": "assistant",
//       "model": "glm-4.7",
//       "content": [
//         {
//           "type": "tool_use",
//           "id": "call_5bcf8991a9a046bd9780bedd",
//           "name": "WebSearch",
//           "input": {
//             "query": "今日黄金价格 2026年2月4日"
//           }
//         }
//       ],
//       "stop_reason": "tool_use",
//       "stop_sequence": null,
//       "usage": {
//         "input_tokens": 16216,
//         "output_tokens": 41,
//         "cache_read_input_tokens": 384,
//         "server_tool_use": {
//           "web_search_requests": 0
//         }
//       }
//     }

#[derive(serde::Deserialize, Debug)]
pub struct ClaudeCodeAssistantContent {
    pub id: String,
    pub role: String,
    pub model: String,
    pub content: [ClaudeCodeMessageContent; 1],
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClaudeCodeMessageContent {
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "text")]
    Text { text: String },
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_deserialize_assistant_message() {
        let json_data = r#"
{
  "parentUuid": "512b8a59-e6e2-4c52-98eb-639f9507d8c7",
  "isSidechain": false,
  "userType": "external",
  "cwd": "/home/csh/my_workspace/speak_term",
  "sessionId": "5b5d4905-ac9a-43e9-ba30-7b71aeceebcf",
  "version": "2.0.76",
  "gitBranch": "main",
  "message": {
    "id": "msg_20260204034521b04dbf504c2a4319",
    "type": "message",
    "role": "assistant",
    "model": "glm-4.7",
    "content": [
      {
        "type": "tool_use",
        "id": "call_a1eb95ef746740859a6b419c",
        "name": "WebSearch",
        "input": {
          "query": "黄金价格 2026年2月4日 今天"
        }
      }
    ],
    "stop_reason": null,
    "stop_sequence": null,
    "usage": {
      "input_tokens": 15964,
      "output_tokens": 44,
      "cache_read_input_tokens": 640,
      "server_tool_use": {
        "web_search_requests": 0
      }
    }
  },
  "type": "assistant",
  "uuid": "5b1183ea-1a68-4993-baea-e7bf36ca8a77",
  "timestamp": "2026-02-03T19:45:23.681Z"
}"#;

        let assistant_message: ClaudeCodeLog = serde_json::from_str(json_data).unwrap();
        println!("{:#?}", assistant_message);
    }

    #[test]
    fn test_file_history_snapshot() {
        let json_data = r#"
{
  "type": "file-history-snapshot",
  "messageId": "3fcd8eb2-9a42-4d3b-a20d-14add25ce14e",
  "snapshot": {
    "messageId": "3fcd8eb2-9a42-4d3b-a20d-14add25ce14e",
    "trackedFileBackups": {},
    "timestamp": "2026-02-03T19:45:19.124Z"
  },
  "isSnapshotUpdate": false
}
        "#;

        let snapshot: ClaudeCodeLog = serde_json::from_str(json_data).unwrap();
        println!("{:#?}", snapshot);
    }

    #[test]
    fn test_user_message() {
        let json_data = r#"
        {
  "parentUuid": null,
  "isSidechain": false,
  "userType": "external",
  "cwd": "/home/csh/my_workspace/speak_term",
  "sessionId": "d81a49c3-8461-4fe1-b315-a26aeedb9dd4",
  "version": "2.0.76",
  "gitBranch": "main",
  "type": "user",
  "message": {
    "role": "user",
    "content": "帮我搜索一下今天白银的价格"
  },
  "uuid": "a58ae98a-ae73-42c5-8939-47798a81902c",
  "timestamp": "2026-02-03T19:56:35.735Z",
  "thinkingMetadata": {
    "level": "high",
    "disabled": false,
    "triggers": []
  },
  "todos": []
}
        "#;

        let user_message: ClaudeCodeLog = serde_json::from_str(json_data).unwrap();
        println!("{:#?}", user_message);
    }

    #[test]
    fn test_user_tool_result_message() {
        let json_data = r#"
    {
  "parentUuid": "df258c14-223d-4eb2-bd83-a90b7eea1d78",
  "isSidechain": false,
  "userType": "external",
  "cwd": "/home/csh/my_workspace/speak_term",
  "sessionId": "d284f444-9e56-4318-a472-bc18481b7793",
  "version": "2.0.76",
  "gitBranch": "main",
  "type": "user",
  "message": {
    "role": "user",
    "content": [
      {
        "type": "tool_result",
        "content": "The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed.",
        "is_error": true,
        "tool_use_id": "call_5bcf8991a9a046bd9780bedd"
      }
    ]
  },
  "uuid": "bb7a7ea8-3028-4318-b27c-44ed53c45372",
  "timestamp": "2026-02-03T18:12:41.873Z",
  "toolUseResult": "Error: The user doesn't want to proceed with this tool use. The tool use was rejected (eg. if it was a file edit, the new_string was NOT written to the file). STOP what you are doing and wait for the user to tell you how to proceed."
}
        "#;

        let user_message: ClaudeCodeLog = serde_json::from_str(json_data).unwrap();
        println!("{:#?}", user_message);
    }
}
