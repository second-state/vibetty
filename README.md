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

### 1. Set Environment Variables

Create a `.env` file and configure the Whisper API (Groq recommended):

```bash
ASR_API_KEY=your_api_key_here
ASR_URL=https://api.groq.com/openai/v1/audio/transcriptions
ASR_MODEL=whisper-large-v3
ASR_LANG=en
ASR_PROMPT=
```

### 2. Start the Service

```bash
# Run directly with cargo
cargo run -- -- claude

# Or build and run
cargo build --release
./target/release/vibetty -- claude
```

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
