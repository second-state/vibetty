# Vibetty

面向 AI 编程 agent 的语音终端。对着 **VibeKeys Max** 键盘说话，Vibetty 会把语音转写后直接送进你选择的编程 agent —— Claude Code、Codex、Gemini CLI —— 或任意终端程序，并通过网页提供访问。

## 工作原理

你对着 VibeKeys Max 键盘的内置麦克风说话（按住说话 / 切换模式）。键盘把音频通过 WebSocket 流式传到 Vibetty 服务端。在默认的 **Whisper** 模式下，服务端把音频打包成 WAV 发送给 Whisper 兼容 API（Groq、OpenAI、GLM…），再把转写出的文本注入终端会话以及其中运行的 AI agent。浏览器页面用于显示实时终端。

```
VibeKeys Max 麦克风 ──WebSocket──▶ Vibetty 服务端 ──WAV──▶ Whisper API ──文本──▶ 终端 + agent
 （按住说话 / 切换）
```

不想用云端 API？**WebVosk** 模式可以完全在浏览器本地做语音识别，无需 API Key，详见 [WebVosk](#webvosk离线无需-api-key)。

## 功能特性

- **WebSocket 终端** - 基于 Axum 框架的实时终端 Web 接口
- **语音输入** - 通过 VibeKeys Max 麦克风说话，语音转写为文本
- **Agent 无关** - 可包裹任意基于终端的编程 agent（Claude Code、Codex、Gemini CLI、OpenCode、aider…）或任意 shell 命令 —— Vibetty 不绑定任何单一 agent
- **多种 ASR 后端**
  - Whisper API —— OpenAI、Groq、GLM、ByteFuture 或任意自定义端点（默认）
  - WebVosk —— 离线、浏览器内、无需 API Key
  - 阿里云 Paraformer 实时语音识别（todo）

## 安装

### 方式 A：下载预编译二进制

从 [releases 页面](https://github.com/second-state/vibetty/releases) 下载对应平台的最新版本：

| 平台 | 文件 |
|---|---|
| Linux | `vibetty-linux-x64` |
| macOS（Apple Silicon） | `vibetty-macos-arm64` |
| Windows | `vibetty-windows-x64.exe` |

### 方式 B：从源码编译

需要安装 [Rust](https://rustup.rs/)。

```bash
cargo build --release
# 二进制位于 ./target/release/vibetty（Windows 上为 vibetty.exe）
```

### 加入 PATH（可选）

如需在任意目录下运行 `vibetty`，把二进制文件放进 `PATH` 中的目录，推荐 `~/.cargo/bin`：

```bash
# 预编译二进制
mv vibetty ~/.cargo/bin/

# 或自行编译的二进制
mv target/release/vibetty ~/.cargo/bin/
```

Windows（PowerShell）：

```powershell
move vibetty-windows-x64.exe $env:USERPROFILE\.cargo\bin\vibetty.exe
```

## 快速开始

Vibetty 默认使用 **Whisper** 模式（云端转写）。最快路径：

**1. 获取 Whisper API Key。** 推荐 Groq，也可使用 OpenAI、GLM、ByteFuture 或任意 Whisper 兼容端点。

**2. 配置 ASR**，运行交互式向导（会写入 `~/.vibetty/config.toml`）：

```bash
vibetty setup
```

手动通过环境变量配置见 [配置](#配置)。

**3. 启动服务并带上你的 agent。** `--` 后面的内容都会在终端里启动，所以 Vibetty 适配任意 coding agent CLI：

| Agent | 启动命令 |
|---|---|
| Claude Code | `vibetty -- claude` |
| OpenAI Codex | `vibetty -- codex` |
| Gemini CLI | `vibetty -- gemini` |
| OpenCode | `vibetty -- opencode` |
| aider | `vibetty -- aider` |
| 纯 shell | `vibetty -- bash` |

对应的 agent CLI 需要已经安装并在你的 `PATH` 中。

**4. 配对 VibeKeys Max。** 打开 `http://localhost:3000/setup`，通过蓝牙连接键盘。把 **VibeKeys 服务器 WebSocket 地址** 设为你的 Vibetty 服务端（例如 `ws://<你的主机>:3000/ws`），并选择**麦克风模式**（PushToTalk 或 Toggle）。

**5. 查看终端。** 打开 `http://localhost:3000` 看到实时会话。对着键盘说话，你的话会作为命令执行。

查看全部参数：

```bash
vibetty --help
```

## 配置

Vibetty 支持两种语音识别后端，可交互式配置，也可通过环境变量配置。

### 交互式配置（推荐）

```bash
vibetty setup
```

进入 TUI 界面，可以：
1. 选择平台：**Whisper** 或 **WebVosk**
2. 如果选择 Whisper，可选择提供商预设：**OpenAI**、**ByteFuture**、**Groq**、**GLM** 或 **Custom**
3. 填写 API Key 等配置项
4. 配置保存到 `~/.vibetty/config.toml`

### Whisper（默认）

服务端通过 Whisper 兼容 API 转写。创建 `.env` 文件（或写入 shell 配置文件，如 `~/.bashrc` / `~/.zshrc`）：

```bash
VIBECODE_ASR_API_KEY=your_api_key_here
VIBECODE_ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
VIBECODE_ASR_MODEL=whisper-large-v3
VIBECODE_ASR_LANG=zh
VIBECODE_ASR_PROMPT=
```

然后启动服务：

```bash
vibetty -- claude
```

### WebVosk（离线，无需 API Key）

语音识别完全在浏览器中使用 Vosk 模型运行，无需 API 密钥，音频也不会发送到云端。

```bash
VIBECODE_ASR_PLATFORM=web_vosk vibetty -- claude
```

然后访问 WebVosk 界面：`http://localhost:3000/vosk`

**注意：** 首次使用需要下载 Vosk 模型文件（每个约 40MB）。模型会缓存在浏览器中。

## 环境变量

| 变量 | 说明 | 默认值 |
|---|---|---|
| `VIBECODE_ASR_PLATFORM` | 使用的 ASR 平台：`whisper` 或 `web_vosk` | `whisper` |
| `VIBECODE_ASR_URL` | Whisper API 端点 URL | `https://api.openai.com/v1/audio/transcriptions` |
| `VIBECODE_ASR_API_KEY` | Whisper API Key（推荐 Groq） | _(空)_ |
| `VIBECODE_ASR_MODEL` | Whisper 模型名 | `whisper-1` |
| `VIBECODE_ASR_LANG` | ASR 语言（如 `en`、`zh`） | _(空，由 API 自动检测)_ |
| `VIBECODE_ASR_PROMPT` | 传给 Whisper API 用于引导转写的提示词 | _(空)_ |
| `VIBECODE_ASR_DEBUG_WAV` | 设为任意值即可把录音保存为 `debug_<session_id>.wav` 用于调试 | _(未设置)_ |
| `VIBECODE_EXIT_COMMAND` | 自定义语音退出命令。当 ASR 结果（不区分大小写）匹配该值时，替换为 `/exit` | _(未设置)_ |

> **注意：** 旧版环境变量（如 `ASR_URL`、`ASR_API_KEY`、`VIBETTY_EXIT_COMMAND` 等）已统一加上 `VIBECODE_` 前缀。使用旧名称会有警告但仍可工作，请尽快迁移到新名称。

## 平台支持

Vibetty 支持 **Linux**、**macOS** 和 **Windows**。

| 平台 | PTY 后端 | 系统要求 |
|---|---|---|
| Linux | Unix PTY | — |
| macOS | Unix PTY | — |
| Windows | ConPTY（基于 [`portable-pty-psmux`](https://crates.io/crates/portable-pty-psmux)） | Windows 10（1809+）或 Windows 11 |

### 在 Windows 上运行

上面的快速开始命令使用 Unix 风格路径；在 Windows 上请在 **PowerShell** 或**命令提示符**中使用 `.exe` 和反斜杠路径：

```powershell
# 预编译二进制
.\vibetty-windows-x64.exe -- claude

# 或从源码编译
cargo build --release
.\target\release\vibetty.exe -- claude
```

在 PowerShell 中用 `$env:` 设置环境变量：

```powershell
$env:VIBECODE_ASR_API_KEY = "your_api_key_here"
$env:VIBECODE_ASR_URL     = "https://api.groq.com/openai/v1/audio/transcriptions"
.\vibetty.exe -- claude
```

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
