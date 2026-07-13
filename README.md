# vibetty

Share an interactive terminal session (`claude`, `codex`) running on your PC with remote devices (ESP32, MCU, another machine) in real time — over MQTT.

vibetty runs a program in a PTY, renders the terminal screen to an image, and publishes it to MQTT. Remote clients subscribe to display the live screen and send keystrokes back. Both ends are just MQTT clients talking to the same broker, so **no port on your PC needs to be exposed** to the public internet.

> 0.4.0 introduces MQTT as the primary sharing transport, alongside an optional HTTP path. Both share one PTY session. MQTT is opt-in: it turns on only when a `[mqtt]` section exists in the config; otherwise MQTT is never touched. See the [changelog](CHANGELOG.md).

## Features

- **Live terminal sharing over MQTT** — the screen is published as JPEG screenshots; remote devices subscribe and send keystrokes back.
- **Built-in broker** — an in-process rumqttd broker (TCP + WebSocket) can auto-start, so you need zero external infrastructure.
- **Multi-instance discovery** — each instance announces itself with a retained presence; a client discovers every one of your instances with a single wildcard subscription.
- **Agent state detection** — the terminal title is parsed to tell whether Codex / Claude Code is `working` or `waiting`, and that state is broadcast alongside presence.
- **TUI controls** — a ratatui interface with `HTTP` / `MQTT` / `Fit` / `Quit` buttons; the `MQTT` button reflects the combined state (`off` / `brkr` / `conn` / `on`).
- **Optional HTTP path** — a `/screenshot` endpoint and an MQTT-over-WebSocket debug page (`/mqtt_ws`), started on demand.
- **Cross-platform** — Linux, macOS, Windows (via ConPTY).

## Quick start

### 1. Install

Download a prebuilt binary for your platform from the [Releases page](https://github.com/second-state/vibetty/releases) and put it on your `PATH` (e.g. `~/.cargo/bin`).

<details>
<summary>Build from source</summary>

```bash
git clone https://github.com/second-state/vibetty
cd vibetty
cargo build --release
# binary: ./target/release/vibetty
```
</details>

### 2. Configure the broker (once)

```bash
vibetty setup
```

This opens a TUI to fill in the `[mqtt]` fields and writes `~/.vibetty/config.toml`. The simplest setup is the built-in broker — no external service needed:

```toml
[mqtt]
enable = true
builtin_broker = true
builtin_port = 1883      # built-in broker TCP port
builtin_ws_port = 9001   # built-in broker WebSocket port
```

Or point at your own broker / a free MQTT cloud:

```toml
[mqtt]
enable = true
broker = "mqtt://user:pass@broker.example.com:1883"   # mqtts:// for TLS
```

> Without a `[mqtt]` section, vibetty does not enable MQTT and never reaches the broker.

### 3. Run

```bash
vibetty -- claude        # share a `claude` session
vibetty -- codex         # share a `codex` session
```

You **must** pass `-- <command>`. The `MQTT` button at the top of the TUI shows `conn` once it has connected and is sharing.

To keep it running in the background without occupying a terminal:

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
tmux capture-pane -t vibetty -p | tail -20   # check it's up
```

Tip: `vibetty skill install --claude` installs a `run-vibetty` skill that hands an agent the full background-session recipe.

## CLI reference

```
vibetty -- <command>                        Run mode: share the given command in a TUI (default)
vibetty setup                               Configure [mqtt] via TUI
vibetty skill install    --claude [--codex] Install the run-vibetty skill
vibetty skill uninstall  --claude [--codex] Remove the run-vibetty skill
```

Run-mode options:

| Flag | Description | Default |
|---|---|---|
| `-- <command>` | Program to run in the PTY (e.g. `-- claude`) | _(required)_ |
| `--config <PATH>` | Override the config file path | `~/.vibetty/config.toml` |
| `-b, --bind-addr <ADDR>` | HTTP listen address (used as the dialog default) | `0.0.0.0:3000` |
| `-a, --auto-submit` | Append Enter to `input_text` so a sent command is executed | `true` |

## HTTP endpoints (started on demand)

The HTTP server is **off by default**; toggle it with the `HTTP` button in the TUI.

- `GET /screenshot` — the current terminal screen as a JPEG image.
- `GET /mqtt_ws` — a browser MQTT-over-WebSocket viewer. It connects to the built-in broker's WS port, discovers instances, shows the screen, and can send input. Handy for testing without hardware.

## Documentation

- **[Detailed usage guide](docs/USAGE.md)** — configuration, the TUI, the full MQTT protocol, and an ESP32 / MCU integration guide. ([中文版](docs/USAGE.zh.md))
- [Changelog](CHANGELOG.md) · [中文更新日志](docs/CHANGELOG.zh-CN.md)

## Platform support

Linux, macOS, and Windows (via ConPTY).
