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

#### Option 1: Whisper API (Server-side)

Create a `.env` file and configure the Whisper API (Groq recommended):

```bash
ASR_API_KEY=your_api_key_here
ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
ASR_MODEL=whisper-large-v3
ASR_LANG=en
ASR_PROMPT=
```

Then start the service:

```bash
# Run directly with cargo
cargo run -- -- claude

# Or build and run
cargo build --release
./target/release/vibetty -- claude
```

#### Option 2: WebVosk (Browser-side)

Speech recognition runs entirely in the browser using Vosk models. No API key required.

```bash
# Set ASR platform to WebVosk
ASR_PLATFORM=web_vosk cargo run -- -- claude
```

Then visit the WebVosk interface at: http://localhost:3000/vosk

**Note:** First-time use requires downloading Vosk model files (~40MB each). The models are cached in your browser.

For more options, use `--help`:
```bash
cargo run -- --help
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
