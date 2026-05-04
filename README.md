# Vibetty

WebSocket terminal server with voice input support and Claude AI intelligent interaction.

## Features

- **WebSocket Terminal** - Real-time terminal web interface based on Axum framework
- **Voice Input** - Speech-to-text support for executing commands via voice
- **Claude AI Integration** - AI-assisted terminal interaction using `echokit_terminal`
- **Multiple ASR Support**
  - OpenAI Whisper API
  - Alibaba Cloud Paraformer real-time speech recognition (todo)

## Quick Start

### ASR Configuration

Vibetty supports two speech recognition modes:

#### Interactive Setup (Recommended)

Run the setup wizard to configure ASR interactively:

```bash
vibetty setup
```

This launches a TUI where you can:
1. Select a platform: **Whisper** or **WebVosk**
2. If Whisper, choose a provider preset: **OpenAI**, **ByteFuture**, **Groq**, **GLM**, or **Custom**
3. Fill in API key and other settings
4. Configuration is saved to `~/.vibetty/config.toml`

#### Manual Configuration

You can also configure ASR manually via environment variables.

##### Option 1: Whisper API (Server-side)

Create a `.env` file and configure the Whisper API (Groq recommended). Alternatively, you can set these environment variables directly in your shell configuration file (e.g., `~/.bashrc` or `~/.zshrc`):

```bash
VIBECODE_ASR_API_KEY=your_api_key_here
VIBECODE_ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
VIBECODE_ASR_MODEL=whisper-large-v3
VIBECODE_ASR_LANG=en
VIBECODE_ASR_PROMPT=
```

Then start the service:

**Option A: Download pre-built binary**

Download the latest release from the [releases page](https://github.com/second-state/vibetty/releases):

```bash
# After downloading
./vibetty -- claude
```

**Option B: Build from source**

```bash
# Build the release binary
cargo build --release
./target/release/vibetty -- claude
```

**Tip:** To run `vibetty` from any directory, place the binary in a directory on your `PATH`. If it exists, we recommend `~/.cargo/bin`:


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


```bash
# For pre-built binary
mv vibetty ~/.cargo/bin/

# Or for self-compiled binary
mv target/release/vibetty ~/.cargo/bin/
```


```bash
# For pre-built binary
mv vibetty ~/.cargo/bin/

# Or for self-compiled binary
mv target/release/vibetty ~/.cargo/bin/
```

##### Option 2: WebVosk (Browser-side)

Speech recognition runs entirely in the browser using Vosk models. No API key required.

**Option A: Download pre-built binary**

```bash
# Set ASR platform and run
VIBECODE_ASR_PLATFORM=web_vosk ./vibetty -- claude
```

**Option B: Build from source**

```bash
# Set ASR platform and run
VIBECODE_ASR_PLATFORM=web_vosk ./vibetty -- -- claude
```

Then visit the WebVosk interface at: http://localhost:3000/vosk

**Note:** First-time use requires downloading Vosk model files (~40MB each). The models are cached in your browser.

For more options, use `--help`:
```bash
./vibetty --help
```

Visit: http://localhost:3000 after starting the service.

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

Currently supports **Linux** and **macOS**. Windows is not supported because the [`pty-process`](https://crates.io/crates/pty-process) library (used for pseudo-terminal handling) is Unix-only and does not support Windows ConPTY.
