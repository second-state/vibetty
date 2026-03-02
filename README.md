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
# Use Groq's Whisper API (recommended, fast)
WHISPER_API_KEY=your_groq_api_key_here
WHISPER_API_URL=https://api.groq.com/openai/v1/audio/transcriptions
WHISPER_MODEL=whisper-large-v3
```

### 2. Start the Service

```bash
# Run directly with cargo
cargo run -- -- claude

# Or build and run
cargo build --release
./target/release/vibetty -- --claude
```

For more options, use `--help`:
```bash
cargo run -- --help
```

Visit: http://localhost:3000 after starting the service.
