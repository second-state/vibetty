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

### ASR 配置

Vibetty 支持两种语音识别模式：

#### 交互式配置（推荐）

运行配置向导来交互式配置 ASR：

```bash
vibetty setup
```

启动后会进入 TUI 界面，可以：
1. 选择平台：**Whisper** 或 **WebVosk**
2. 如果选择 Whisper，可以选择提供商预设：**OpenAI**、**ByteFuture**、**Groq**、**GLM** 或 **Custom**
3. 填写 API Key 等配置项
4. 配置保存到 `~/.vibetty/config.toml`

#### 手动配置

也可以通过环境变量手动配置 ASR。

##### 选项 1：Whisper API（服务器端）

创建 `.env` 文件并配置 Whisper API（推荐使用 Groq）：

```bash
VIBECODE_ASR_API_KEY=your_api_key_here
VIBECODE_ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
VIBECODE_ASR_MODEL=whisper-large-v3
VIBECODE_ASR_LANG=zh
VIBECODE_ASR_PROMPT=
```

然后启动服务：

```bash
# 使用 cargo 直接运行
cargo run -- -- claude

# 或者先编译再运行
cargo build --release
./target/release/vibetty -- claude
```

##### 选项 2：WebVosk（浏览器端）

语音识别完全在浏览器中使用 Vosk 模型运行，无需 API 密钥。

```bash
# 设置 ASR 平台为 WebVosk
VIBECODE_ASR_PLATFORM=web_vosk cargo run -- -- claude
```

然后访问 WebVosk 界面：http://localhost:3000/vosk

**注意：** 首次使用需要下载 Vosk 模型文件（每个约 40MB）。模型会缓存在浏览器中。

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

## 平台支持

Vibetty 支持 **Linux**、**macOS** 和 **Windows**。

| 平台 | PTY 后端 | 系统要求 |
|---|---|---|
| Linux | Unix PTY | — |
| macOS | Unix PTY | — |
| Windows | ConPTY（基于 [`portable-pty-psmux`](https://crates.io/crates/portable-pty-psmux)） | Windows 10（1809+）或 Windows 11 |

### 在 Windows 上运行

预编译版本包含名为 `vibetty-windows-x64.exe` 的 Windows 二进制文件。上面的快速开始命令使用的是 Unix 风格路径；在 Windows 上请在 **PowerShell** 或**命令提示符**中使用 `.exe` 和反斜杠路径：

```powershell
# 预编译二进制
.\vibetty-windows-x64.exe -- claude

# 或从源码编译
cargo build --release
.\target\release\vibetty.exe -- claude
```

在 PowerShell 中可用 `$env:` 设置环境变量：

```powershell
$env:VIBECODE_ASR_API_KEY = "your_api_key_here"
$env:VIBECODE_ASR_URL     = "https://api.groq.com/openai/v1/audio/transcriptions"
.\vibetty.exe -- claude
```

如需在任意目录下运行 `vibetty`，请将二进制文件移动到 `PATH` 中的目录（例如 `%USERPROFILE%\.cargo\bin`）：

```powershell
move vibetty.exe $env:USERPROFILE\.cargo\bin\
```
