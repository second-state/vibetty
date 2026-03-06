# Vibetty

WebSocket 终端服务器，支持语音输入和 Claude AI 智能交互。

## 功能特性

- **WebSocket 终端** - 基于 Axum 框架的实时终端 Web 接口
- **语音输入** - 支持语音转文字，可通过语音执行命令
- **Claude AI 集成** - 使用 `echokit_terminal` 实现 AI 辅助终端交互
- **多种 ASR 支持**
  - OpenAI Whisper API
  - 阿里云 Paraformer 实时语音识别(todo)

## 快速开始

### 1. 设置环境变量

创建 `.env` 文件并配置 Whisper API（推荐使用 Groq）：

```bash
# 使用 Groq 的 Whisper API（推荐，速度快）
WHISPER_API_KEY=your_groq_api_key_here
WHISPER_API_URL=https://api.groq.com/openai/v1/audio/transcriptions
WHISPER_MODEL=whisper-large-v3
```

### 2. 启动服务

```bash
# 使用 cargo 直接运行
cargo run -- -- claude

# 或者先编译再运行
cargo build --release
./target/release/vibetty -- -- claude
```

更多参数可以使用 `--help` 命令查看：
```bash
cargo run -- --help
```

服务启动后访问: http://localhost:3000

## API 参考

### 更改目录

通过 HTTP API 更改当前工作目录。

**接口地址：** `POST /api/change-dir`

**请求格式：**
```bash
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "/path/to/directory"}'
```

**使用示例：**
```bash
# 切换到绝对路径
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "/home/user/documents"}'

# 切换到相对路径
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "../parent-folder"}'
```

**注意：** 出于安全考虑，此接口仅接受来自 localhost 的请求。
