# Vibetty

Voice-driven terminal for AI coding agents. Speak into a **VibeKeys Max** keyboard and Vibetty transcribes your speech straight into Claude Code (or any terminal program), served over the web.

## How it works

You speak into the VibeKeys Max keyboard's built-in microphone (push-to-talk or toggle). The keyboard streams the audio over a WebSocket to the Vibetty server. In the default **Whisper** mode, the server packages the audio as WAV and sends it to a Whisper-compatible API (Groq, OpenAI, GLM…), then injects the transcribed text into your terminal session and the AI agent running in it. A browser view shows the live terminal.

```
VibeKeys Max mic ──WebSocket──▶ Vibetty server ──WAV──▶ Whisper API ──text──▶ terminal + agent
 (push-to-talk / toggle)
```

Prefer not to use a cloud API? **WebVosk** mode runs speech recognition locally in the browser instead, with no API key. See [WebVosk](#webvosk--offline-no-api-key).

## Features

- **WebSocket Terminal** - Real-time terminal web interface based on the Axum framework
- **Voice Input** - Speak commands through the VibeKeys Max microphone; speech is transcribed to text
- **Claude AI Integration** - AI-assisted terminal interaction
- **Multiple ASR Backends**
  - Whisper API — OpenAI, Groq, GLM, ByteFuture, or any custom endpoint (default)
  - WebVosk — offline, in-browser, no API key
  - Alibaba Cloud Paraformer real-time recognition (todo)

## Installation

### Option A: Download a pre-built binary

Download the latest release for your platform from the [releases page](https://github.com/second-state/vibetty/releases):

| Platform | Asset |
|---|---|
| Linux | `vibetty-linux-x64` |
| macOS (Apple Silicon) | `vibetty-macos-arm64` |
| Windows | `vibetty-windows-x64.exe` |

### Option B: Build from source

```bash
cargo build --release
# binary at ./target/release/vibetty
```

### Add to your PATH (optional)

To run `vibetty` from any directory, place the binary in a directory on your `PATH`. We recommend `~/.cargo/bin`:

```bash
# Pre-built binary
mv vibetty ~/.cargo/bin/

# Or self-compiled binary
mv target/release/vibetty ~/.cargo/bin/
```

On Windows (PowerShell):

```powershell
move vibetty-windows-x64.exe $env:USERPROFILE\.cargo\bin\vibetty.exe
```

<details>
<summary>What is PATH?</summary>

`PATH` is an environment variable that tells your shell which directories to search for executable programs. When you type a command like `ls` or `cargo`, the shell looks through each directory in `PATH` (in order) until it finds a matching executable.

For example, if your `PATH` is:

```bash
/usr/local/bin:/usr/bin:/bin:/home/user/.cargo/bin
```

When you run `vibetty`, the shell searches:
1. `/usr/local/bin/vibetty` (not found)
2. `/usr/bin/vibetty` (not found)
3. `/bin/vibetty` (not found)
4. `/home/user/.cargo/bin/vibetty` (found!) ← executes this

To check your current `PATH`:

```bash
echo $PATH
```

To see if a directory is on your `PATH`:

```bash
echo $PATH | grep -q "$HOME/.cargo/bin" && echo "Yes" || echo "No"
```
</details>

## Quick Start

Vibetty defaults to **Whisper** mode (cloud transcription). The fastest path:

**1. Get a Whisper API key.** Groq is recommended. You can also use OpenAI, GLM, ByteFuture, or any Whisper-compatible endpoint.

**2. Configure ASR** with the interactive wizard (writes `~/.vibetty/config.toml`):

```bash
vibetty setup
```

Manual environment-variable configuration is covered in [Configuration](#configuration).

**3. Start the server with your agent:**

```bash
vibetty -- claude
```

**4. Pair your VibeKeys Max.** Open `http://localhost:3000/setup` and connect to the keyboard over Bluetooth. Set the **VibeKeys server WebSocket URL** to your Vibetty server (e.g. `ws://<your-host>:3000/ws`) and choose a **Microphone Mode** (PushToTalk or Toggle).

**5. Watch the terminal.** Open `http://localhost:3000` to see the live session. Speak into the keyboard and your words run as commands.

For all options:

```bash
vibetty --help
```

## Configuration

Vibetty supports two speech-recognition backends. Configure either interactively or via environment variables.

### Interactive setup (recommended)

```bash
vibetty setup
```

A TUI walks you through:
1. Select a platform: **Whisper** or **WebVosk**
2. If Whisper, choose a provider preset: **OpenAI**, **ByteFuture**, **Groq**, **GLM**, or **Custom**
3. Fill in the API key and other settings
4. Settings are saved to `~/.vibetty/config.toml`

### Whisper (default)

Server-side transcription via a Whisper-compatible API. Create a `.env` file (or set these in your shell profile, e.g. `~/.bashrc` / `~/.zshrc`):

```bash
VIBECODE_ASR_API_KEY=your_api_key_here
VIBECODE_ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
VIBECODE_ASR_MODEL=whisper-large-v3
VIBECODE_ASR_LANG=en
VIBECODE_ASR_PROMPT=
```

Then start the service:

```bash
vibetty -- claude
```

### WebVosk — offline, no API key

Speech recognition runs entirely in the browser using Vosk models. No API key required, and no audio is sent to a cloud service.

```bash
VIBECODE_ASR_PLATFORM=web_vosk vibetty -- claude
```

Then open the WebVosk interface at `http://localhost:3000/vosk`.

**Note:** First-time use downloads Vosk model files (~40MB each). The models are cached in your browser.

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `VIBECODE_ASR_PLATFORM` | ASR platform to use: `whisper` or `web_vosk` | `whisper` |
| `VIBECODE_ASR_URL` | Whisper API endpoint URL | `https://api.openai.com/v1/audio/transcriptions` |
| `VIBECODE_ASR_API_KEY` | Whisper API key (Groq recommended) | _(empty)_ |
| `VIBECODE_ASR_MODEL` | Whisper model name | `whisper-1` |
| `VIBECODE_ASR_LANG` | ASR language (e.g. `en`, `zh`) | _(empty, auto-detected by API)_ |
| `VIBECODE_ASR_PROMPT` | Prompt passed to the Whisper API to guide transcription | _(empty)_ |
| `VIBECODE_ASR_DEBUG_WAV` | Set to any value to save recorded audio as `debug_<session_id>.wav` for debugging | _(unset)_ |
| `VIBECODE_EXIT_COMMAND` | Custom voice exit command. When ASR result matches this value (case-insensitive), it is replaced with `/exit` | _(unset)_ |

> **Note:** Legacy environment variables (e.g. `ASR_URL`, `ASR_API_KEY`, `VIBETTY_EXIT_COMMAND`, etc.) have been renamed with the `VIBECODE_` prefix. Using old names will trigger a warning but still work. Please migrate to the new names.

## Platform Support

Vibetty runs on **Linux**, **macOS**, and **Windows**.

| Platform | PTY backend | Requirements |
|---|---|---|
| Linux | Unix PTY | — |
| macOS | Unix PTY | — |
| Windows | ConPTY (via [`portable-pty-psmux`](https://crates.io/crates/portable-pty-psmux)) | Windows 10 (1809+) or Windows 11 |

### Running on Windows

The quick-start commands above use Unix-style paths; on Windows, use the `.exe` and backslash paths from **PowerShell** or **Command Prompt**:

```powershell
# Pre-built binary
.\vibetty-windows-x64.exe -- claude

# Or build from source
cargo build --release
.\target\release\vibetty.exe -- claude
```

Set environment variables with `$env:` in PowerShell:

```powershell
$env:VIBECODE_ASR_API_KEY = "your_api_key_here"
$env:VIBECODE_ASR_URL     = "https://api.groq.com/openai/v1/audio/transcriptions"
.\vibetty.exe -- claude
```

## API Reference

### Change Directory

Change the current working directory via HTTP API.

**Endpoint:** `POST /api/change-dir`

**Request:**
```bash
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "/path/to/directory"}'
```

**Example:**
```bash
# Change to absolute path
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "/home/user/documents"}'

# Change to relative path
curl -X POST http://localhost:3000/api/change-dir \
  -H "Content-Type: application/json" \
  -d '{"path": "../parent-folder"}'
```

**Note:** This endpoint only accepts requests from localhost for security reasons.
