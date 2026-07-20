# vibetty Usage Guide

> 中文版：[USAGE.zh.md](USAGE.zh.md)

This is the complete vibetty documentation: installation, configuration, running, TUI operation, HTTP endpoints, the **full MQTT protocol specification**, an **ESP32 / MCU** integration guide, and debugging tips.
>
> Protocol details are sourced exclusively from `src/mqtt.rs` in the repository; this document is a readable snapshot of it.

## Table of contents

- [1. What vibetty is](#1-what-vibetty-is)
- [2. Installation](#2-installation)
- [3. Configuring MQTT (`vibetty setup`)](#3-configuring-mqtt-vibetty-setup)
- [4. Running](#4-running)
- [5. CLI and configuration reference](#5-cli-and-configuration-reference)
- [6. MQTT transport in detail (protocol spec)](#6-mqtt-transport-in-detail-protocol-spec)
- [7. ESP32 / MCU integration guide](#7-esp32--mcu-integration-guide)
- [8. The `skill` subcommand](#8-the-skill-subcommand)
- [9. Debugging and FAQ](#9-debugging-and-faq)
- [10. Relevant source files](#10-relevant-source-files)

---

## 1. What vibetty is

vibetty runs a program (`claude`, `codex`) in a PTY, **renders the terminal screen to an image**, and publishes it over **MQTT**. Remote devices (ESP32 / MCU / another machine) subscribe to that image to display the live screen and send keystrokes back.

Both ends are just MQTT clients talking to the **same broker**, so:

- **No port on your PC needs to be exposed** — no port mapping, no forwarding server.
- The broker can be **self-hosted** (built-in rumqttd / external mosquitto / EMQX) or a **free MQTT cloud service** (EMQX Cloud, etc.). Your data stays in your own hands.

Each vibetty instance also publishes a **presence** (online announcement), so a remote device can **discover** which of your instances are currently online with a wildcard subscription.

```
        ┌─────────────┐   PTY    ┌──────────────────────────┐
program │  vibetty    │ ───────► │ render terminal → JPEG    │
(claude)│  (PC, TUI)  │          │ publish {P}/screen        │ ┐
        └─────┬───────┘          │ publish {P}  (presence)   │ │
              │ recv {P}/pty_in, └──────────────┬────────────┘ │
              │      {P}/control                ▼              │ MQTT
              │                          ┌─────────────┐       │
              └──────────────────────────│   broker    │◄──────┘
                                         └──────┬──────┘
                                                │
                                     ┌──────────┴──────────┐
                                     ▼                     ▼
                              ┌──────────────┐      ┌────────────────┐
                              │   ESP32/MCU   │      │ browser debug  │
                              │ (sub screen,  │      │ /mqtt_ws       │
                              │  pub pty_in)  │      └────────────────┘
                              └──────────────┘
```

> As of 0.4.0, MQTT is the primary sharing transport; an **optional HTTP path** (`/screenshot` for images, `/mqtt_ws` debug page) is also kept. Both share one PTY session.

---

## 2. Installation

**Option A: download a prebuilt binary (recommended, fastest)**

Download the prebuilt binary for your platform from [Releases](https://github.com/second-state/vibetty/releases) and place it in a directory on your `PATH` (`~/.cargo/bin` is recommended).

**Option B: build from source**

```bash
git clone https://github.com/second-state/vibetty
cd vibetty
cargo build --release
# binary: ./target/release/vibetty
```

Verify:

```bash
vibetty --help
vibetty --version
```

---

## 3. Configuring MQTT (`vibetty setup`)

MQTT is enabled only when the config file **contains a `[mqtt]` section**; otherwise vibetty never touches MQTT. The config file defaults to `~/.vibetty/config.toml` (override with `--config <PATH>`).

### Interactive configuration (recommended)

```bash
vibetty setup
```

Opens a ratatui TUI that edits every field of `[mqtt]` and writes it back to the config (other sections are preserved).

### Manual configuration

Edit `~/.vibetty/config.toml` directly. Three typical setups:

**Setup 1: built-in broker (simplest, zero external dependencies)**

```toml
[mqtt]
enable = true
builtin_broker = true
builtin_port = 1883      # built-in broker TCP port
builtin_ws_port = 9001   # built-in broker WebSocket port
```

On startup, vibetty launches an in-process rumqttd broker; its own client connects to the local `mqtt://127.0.0.1:1883`, and the ESP32 connects directly to your PC's `1883` (the ESP32 must be able to reach the PC, e.g. on the same LAN). The built-in broker is anonymous and listens on `0.0.0.0` — **for internal use only; do not expose it to the public internet**.

**Setup 2: self-hosted external broker (mosquitto / EMQX / rumqttd)**

```toml
[mqtt]
enable = true
broker = "mqtt://username:password@your-broker-host:1883"
```

The broker credentials are written directly in the URL (`mqtt://user:pass@host:port`). Use `mqtts://` for TLS (default port 8883, TLS enabled automatically).

**Setup 3: free MQTT cloud service**

```toml
[mqtt]
enable = true
broker = "mqtts://user:pass@broker.emqx.io:8883"
```

For a single-user scenario, registering a free MQTT cloud service is enough.

> Even with the built-in broker enabled, if `broker` is set, the client connects to that address (it is not forced to local). Only when `broker` is empty **and** `builtin_broker = true` does it default to the local `mqtt://127.0.0.1:{builtin_port}`.

### `[mqtt]` field reference

| Field | Type | Default | Description |
|---|---|---|---|
| `enable` | bool | `true` | Whether to auto-start the MQTT transport client on process startup (turn off to keep the config without connecting). |
| `broker` | string | _(empty)_ | Broker URL: `mqtt://[user:pass@]host:port` or `mqtts://...`. Credentials/TLS/port are all parsed from the URL. |
| `qos` | u8 | `1` | Reserved field. Not currently effective: inbound QoS is hardcoded (`pty_in=0`, `control=1`). |
| `keep_alive_secs` | u64 | `30` | MQTT keep-alive seconds. |
| `builtin_broker` | bool | `false` | Whether to auto-start the built-in rumqttd broker on process startup. |
| `builtin_port` | u16 | `1883` | Built-in broker TCP port. |
| `builtin_ws_port` | u16 | `9001` | Built-in broker WebSocket port (the `/mqtt_ws` debug page connects here). |

> There are no separate `username`/`password` fields: credentials go in the `broker` URL (`mqtt://user:pass@host`).

---

## 4. Running

### 4.1 Basic usage

```bash
vibetty -- claude        # share a claude session
vibetty -- codex         # share a codex session
```

⚠️ **You must pass `-- <command>`**. If you just run `vibetty`, the PTY has no program to run and immediately hits EOF, so vibetty exits right away — easily mistaken for a port conflict or a startup failure.

After launch you enter a ratatui TUI: the screen shows the shared terminal, and the **first row at the top** is the button row `HTTP | MQTT | Fit | Quit`.

- The `MQTT` button text reflects the combined state: `off` (neither client nor broker running) / `brkr` (only the broker is running) / `conn` (client connected) / `on` (both broker and client running).
- `conn` means it has connected to the broker and is sharing the screen.

### 4.2 Running in the background (tmux)

Keep the sharing running in the background without occupying your terminal:

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
sleep 6
tmux ls                                            # should list a session named vibetty
tmux capture-pane -t vibetty -p | tail -20         # confirm claude is up and the MQTT button shows conn
```

- `-s vibetty`: the tmux session name; customize as you like. To share multiple sessions at once, give each a distinct name.
- `-c "<dir>"`: the working directory for the terminal session. **Use `-c`, not `cd ... && tmux`** (the latter does not switch directories as expected when a tmux server is already running).
- `tmux attach -t vibetty` enters the session to view/operate; `Ctrl-b d` detaches (the session keeps running); `tmux kill-session -t vibetty` ends it.

### 4.3 TUI operation (top buttons + MqttPanel)

The top button row (first screen row) `HTTP | MQTT | Fit | Quit` is mouse-clickable and highlights on hover:

- **HTTP**: start/stop the HTTP server on demand (off by default). Clicking it lets you confirm the listen address (prefilled from `--bind-addr`).
- **MQTT**: opens the **MqttPanel** popup, split into two blocks:
  - **Broker block**: the `TCP:` / `WS :` ports are editable (Enter writes back to config); `Start broker` launches the built-in rumqttd (⚠️ **start-only, no stop** — rumqttd has no shutdown API; once started it becomes read-only `● broker running :{port}`).
  - **Client block**: the `URL:` broker address is editable (Enter writes back to `[mqtt] broker`); `Start client` / `Stop client` start/stop the transport client.
  - `Tab` / `↑↓` cycle through the items; Enter's behavior depends on the focused item (port = save, BrokerStart = start broker, URL = save URL, ClientToggle = toggle client).
- **Fit**: one click resets the terminal size to the current window size (the usable area after subtracting the button row and the top border).
- **Quit**: exit the TUI directly.

> Messages are not buffered while the client is stopped and are not replayed on restart; real teardown relies on the **LWT** (empty retained): once the connection drops, the broker automatically clears the presence.

### 4.4 HTTP endpoints (started on demand)

The HTTP server is **off by default**; start it with the `HTTP` button (listen address prefilled from `-b/--bind-addr`, default `0.0.0.0:3000`):

- `GET /screenshot` — returns the current terminal screen as an image (JPEG). Includes `Cache-Control: no-cache`, so you can poll for the latest frame.
- `GET /mqtt_ws` — a browser MQTT-over-WebSocket debug page: uses mqtt.js to connect to the **built-in broker's WS port** (default `:9001`), auto-discovers instances, shows the screen image, and can send input. **The easiest way to test without ESP32 hardware.**

> ⚠️ These endpoints exist only after you explicitly start the HTTP server; otherwise there is no external HTTP service at all.

---

## 5. CLI and configuration reference

### Subcommands

```
vibetty -- <command>                      Run mode: share the given command in a TUI (default)
vibetty setup                             Configure [mqtt] via TUI
vibetty skill install    --claude [--codex] Install the run-vibetty skill
vibetty skill uninstall  --claude [--codex] Remove the run-vibetty skill
```

### Run-mode options

| Flag | Description | Default |
|---|---|---|
| `-- <command>` | Program to run in the PTY (e.g. `-- claude`, `-- codex`). **Required.** | _(required)_ |
| `--config <PATH>` | Override the config file path. | `~/.vibetty/config.toml` |
| `-b, --bind-addr <ADDR>` | HTTP listen address (used as the dialog default when starting HTTP). | `0.0.0.0:3000` |
| `-a, --auto-submit` | Append Enter to `input_text` and execute it (also sets scrollback to 3, exposing a bit of history); turn off to only type the text with scrollback=0 (latest). | `true` |
| `-q, --quality <QUALITY>` | Screen output format: `text` (default; plain ANSI text stream on `P/screen_text`, no image — see §6.5b), or `high` (JPEG q85 color), `medium` (JPEG q70 color), `low` (JPEG q50 grayscale). | `text` |

### Screenshot rendering parameters (fixed)

The terminal screen is rendered to an image with these parameters (the `SCREEN_*` constants, see `src/ws.rs`):

- Character cell: **8 × 18 pixels** (width × height).
- Padding: **16 pixels** on each side.
- Total image size = `cols × 8 + 32` (width) × `rows × 18 + 32` (height).
- Encoded as JPEG; quality tier is set by `-q, --quality` (see Run-mode options above): `high` = q85 color, `medium` = q70 color, `low` = q50 grayscale. The default is `-q text`, which sends the screen as an ANSI text stream instead of an image (no JPEG encoding).

---

## 6. MQTT transport in detail (protocol spec)

> This section is the contract that remote clients (ESP32 / MCU / any MQTT client) must follow. When in doubt, refer to the actual code in `src/mqtt.rs`.

### 6.1 Broker connection

- vibetty is an MQTT client that connects to the broker specified by `[mqtt] broker`.
- **The remote device must connect to the same broker** (same host/port/credentials).
- Authentication: uses the broker username/password (written in vibetty's `broker` URL). The remote uses the same account (or another account on the broker with permission).
- Ports: `1883` plaintext / `8883` TLS (`mqtts://` enables TLS automatically).
- Maximum packet **1MB** (both the client and the built-in broker use this limit; an external broker must also be ≥ 1MB, otherwise the `screen` image gets truncated).

### 6.2 Topic naming (important ⚠️)

All topics of a vibetty instance live under an **automatically constructed prefix**:

```
{user}/{device}/{pid}/vibetty
```

| Segment | Source | Nature |
|---------|--------|--------|
| `user` | The `username` in the broker URL; falls back to **`root`** if no account is set | Stable (multi-tenant isolation) |
| `device` | First 16 hex chars of `SHA256(machine-uid)` (the PC's machine fingerprint) | Stable (survives restarts) + unique across machines |
| `pid` | The vibetty process pid | **Changes on every restart** |

**Why the remote must do discovery**: `device` is the PC's machine fingerprint (the remote cannot compute it) and `pid` changes every time (the remote cannot predict it). So the remote **cannot know an instance's topic in advance** and must discover it via presence first (see 6.7).

### 6.3 Topic list

Let `P = {user}/{device}/{pid}/vibetty` (the instance prefix):

| Direction | Topic | Payload | QoS / retained | Description |
|-----------|-------|---------|----------------|-------------|
| Device → vibetty | `P/pty_in` | **raw bytes** | QoS 0 / no | Raw keystroke bytes (single key / escape sequence) |
| Device → vibetty | `P/control` | **JSON** | QoS 1 / no | Control messages (text input / sync / scroll) |
| vibetty → device | `P/screen` | **raw bytes** (JPEG + 4-byte offset at the end) | QoS 0 / **yes** | A full JPEG screen frame. **JPEG mode only** (`-q high` / `medium` / `low`). |
| vibetty → device | `P/screen_text` | **raw bytes** (`[1-byte tag] + ANSI text`, see 6.5b) | QoS 0 / full-frame **yes**, delta **no** | Text-mode screen stream (full-frame baseline + realtime pty delta). **Text mode only** (`-q text`). |
| vibetty → device | `P` (the prefix itself) | **JSON** | QoS 1 / **yes** | Presence announcement (service discovery) |

> **Two output modes** (`-q`, set at vibetty startup, fixed for the whole session): the instance publishes **either** `P/screen` (JPEG, `-q high/medium/low`) **or** `P/screen_text` (text, `-q text`), never both. The remote picks which to subscribe to based on the presence `format` field (see 6.7). There is **no separate `P/pty_out` topic** — in text mode the realtime pty bytes are carried inside `P/screen_text` as delta frames (tag `0x01`).
>
> `screen` and the full-frame `screen_text` are **retained**: the remote receives the most recent frame immediately upon subscribing. `screen_text` **delta** frames are not retained (so the retained message is always a complete baseline frame).

### 6.4 `control` JSON format

Reuses vibetty's `ClientMessage` serde form (`#[serde(tag="type", content="data")]`, distinguished by `type`). The remote only needs to send these 4:

| type | data | Meaning |
|------|------|---------|
| `input_text` | string | Type a piece of text (e.g. a command). If the server has `--auto-submit` on, Enter is appended and the command is executed. |
| `sync` | `{"width":W,"height":H,"pixels":bool,"close":bool}` | Declare the remote's display size + control autonomous push. `pixels` (default `true`): `width`/`height` are **pixels** → server converts to cols/rows; `false`: already character cols/rows, used directly. `close` (default `false`): `true` = **pause** the server's autonomous screen push (saves bandwidth when the remote isn't viewing); `false` = resume. Server resizes the PTY and replies with a full screen frame. See 6.6. |
| `scroll_up` | `{"rows":N}` | Scroll up; `rows`=0 / omitted = scroll a full page (= the number of visible terminal rows, minus 2 rows of overlap). |
| `scroll_down` | `{"rows":N}` | Scroll down; same as above. |

`sync`'s `pixels` and `close` are **optional and backward-compatible** (omitted ⇒ `pixels:true`, `close:false`). Examples:

```json
{"type":"input_text","data":"ls -la\n"}
{"type":"sync","data":{"width":320,"height":240}}
{"type":"sync","data":{"width":80,"height":24,"pixels":false}}
{"type":"sync","data":{"width":80,"height":24,"pixels":false,"close":true}}
{"type":"scroll_up","data":{"rows":0}}
```

> Difference between `pty_in` (raw single key) and `control`'s `input_text` (text string): **single keys / arrow keys / control characters** go via `pty_in` raw bytes; **whole text / command lines** go via `control`'s `input_text`.

### 6.5 `screen` payload (JPEG mode)

A full JPEG image as bytes, **no chunking, no signaling fields**. But **4 bytes are appended at the end**, which you must account for:

1. The image starts with the JPEG magic bytes `FF D8 FF`; just decode it as JPEG.
2. After decoding, **read the last 4 bytes** as the `scrollback offset` (u32, **big-endian** = network order):
   - `0` = this image was captured at the **bottom / latest** position;
   - `> 0` = captured after scrolling up N rows.
   - These 4 bytes come after the image's `EOI` (JPEG), so the decoder ignores them and decoding is unaffected.

### 6.5b `screen_text` payload (text mode)

Text mode (`-q text`) carries the screen as an **ANSI terminal stream** on `P/screen_text` — a "full-frame baseline + realtime delta" design. Every payload starts with a **1-byte tag**:

```
payload = [ tag: 1 byte ] [ content ]
```

| tag | meaning | content | retained | when |
|-----|---------|---------|----------|------|
| `0x00` | **full-frame baseline** | vt300 `contents_formatted()` — a replayable ANSI stream (cursor-home + SGR colors + text); feed it to an empty terminal parser to reproduce the whole screen (with colors) | **yes** | startup first frame, and in response to every `sync` / `scroll_*` |
| `0x01` | **pty delta** (incremental) | raw PTY output bytes (ANSI escapes + text) | **no** | every time the PTY produces output (realtime) |

- **Why delta is not retained**: so the broker's retained message is always a complete `0x00` baseline. On reconnect the remote gets a usable full frame, not a stale delta that would produce garbage on a blank buffer.
- **Recommended remote implementation**: keep a terminal-emulator buffer; on `0x00` reset the buffer and replay the full frame; on `0x01` feed the bytes in as incremental output (exactly like a real terminal receiving shell output). On connect, the retained `0x00` gives the baseline; if unsure, send a `sync` to force a fresh `0x00`.
- The content **contains ANSI escape codes** (`\x1b[...`) — it is not plain trimmed text. Either render it through a terminal parser or strip the escapes yourself.
- `close=true` stops the `0x01` deltas (autonomous push paused); `0x00` full frames still come in response to `sync` / `scroll_*`.

### 6.6 `sync` size conversion + `close` switch

**Size units** depend on the `pixels` field:

- `pixels: true` (default): `width`/`height` are **pixels**. The server converts them using the screenshot rendering parameters (see section 5):

  ```
  cols = (width  - 32) / 8      # 32 = 16px padding on each side; 8 = char width
  rows = (height - 32) / 18     # 18 = char height
  ```

- `pixels: false`: `width`/`height` are already **character columns/rows**; the server uses them directly.

Minimum `cols = 8`, `rows = 2` (to avoid a vt100 0-row panic). The remote just reports its own display size truthfully and the server resizes the PTY accordingly.

**`close` switch** (bandwidth saver): controls the server's **autonomous** screen push (the frames triggered by PTY output):

| `close` | JPEG mode | Text mode |
|---------|-----------|-----------|
| `false` (default) | PTY output settles ≥ 100ms → send a `P/screen` frame | every PTY output → send a `P/screen_text` delta (`0x01`) |
| `true` | stop sending `P/screen` (in-flight frame dropped too) | stop sending deltas (`0x01`) |

Unaffected by `close` (client-initiated, always answered): the `sync` screen reply, `scroll_*` replies, and the presence heartbeat. Typical use: when the remote's display is off / the user isn't looking, send `close=true` to mute the stream; send `close=false` to resume.

### 6.7 Discovery / presence mechanism

On startup, vibetty publishes a **retained** presence on `P` (the prefix itself):

```json
{
  "prefix": "root/1a2b3c4d5e6f7a8b/12345/vibetty",
  "client_id": "vibetty-1a2b3c4d5e6f7a8b-12345",
  "ts": 1751300000,
  "title": "claude — workspace",
  "state": "working",
  "format": "high"
}
```

| Field | Meaning |
|-------|---------|
| `prefix` | The full instance prefix (the remote subscribes to output channels based on this) |
| `client_id` | vibetty's MQTT client id (for debugging) |
| `ts` | Current epoch seconds (the remote uses this for liveness) |
| `title` | The terminal window title (set by the program via OSC), used for agent state detection |
| `state` | The agent working state: `"working"` or `"waiting"` (lowercase). Codex / Claude Code are `waiting` when waiting for user action. |
| `format` | **Output mode**: `"high"` / `"medium"` / `"low"` (JPEG → subscribe `P/screen`) or `"text"` (text → subscribe `P/screen_text`). Decide which screen topic to subscribe to from this. |

- **Re-sent every 15s** (heartbeat, refreshes `ts`).
- **Abnormal disconnect**: the broker triggers the LWT and publishes an **empty payload** to `P` (= deletes the retained message), so the remote immediately knows the instance went offline.
- **Agent state transition** (working↔waiting) **immediately re-publishes** presence, so the remote can decide whether to push the screen to the user.

**Remote discovery subscription**:

- If the remote knows `user` (= its own broker username, and vibetty uses the same one): subscribe `{user}/+/+/vibetty` (`+` wildcards the device and pid segments).
- If it does not know user (vibetty has no account in the broker URL, so the user segment falls back to `root`): use the broader `+/+/+/vibetty` (wildcards all three of user/device/pid).
- Retained messages guarantee the remote **immediately receives all existing instances'** presence upon connecting.

---

## 7. ESP32 / MCU integration guide

This section is for anyone working in the ESP32 repo. The goal: let the ESP32 connect to vibetty on the PC over MQTT to "discover instances + display the screen + send/receive input".

### 7.1 Feature checklist

1. ✅ Connect to the broker (host/port/auth consistent with vibetty's config).
2. ✅ **Discovery**: subscribe to the presence wildcard topic, parse payloads, maintain an "online instance list".
3. ✅ After choosing a target instance, subscribe to the screen topic **based on its `format`**: `"text"` → `{P}/screen_text`; otherwise (JPEG) → `{P}/screen`. (If you don't need the screen, skip subscribing to save bandwidth and only send input.)
4. ✅ **JPEG mode**: receive `screen` → detect JPEG by magic bytes → decode and display → read the last 4 bytes for the scrollback offset. **Text mode**: receive `screen_text` → read the first byte (`0x00` = full-frame baseline → reset your terminal buffer and replay; `0x01` = pty delta → feed incrementally to the buffer).
5. ✅ Send single keys → publish `{P}/pty_in` (raw bytes).
6. ✅ Send text commands → publish `{P}/control` (JSON `input_text`).
7. ✅ Report the display size (+ bandwidth switch) → publish `{P}/control` (JSON `sync` with `width`/`height`/`pixels`/`close`).
8. ✅ **Liveness check**: presence `ts` (treat > ~30s without update as offline) + LWT empty payload (instance offline, remove immediately).
9. ✅ **Switch target**: unsubscribe the old instance's screen topic (`screen` or `screen_text`), subscribe the new one's (per its `format`).
10. ✅ (Optional) **Agent state**: use presence's `state` to decide whether to push the screen to the user.
11. ✅ (Optional, **bandwidth saver**) **`close` switch**: when the display is off / user isn't viewing, send `sync` with `close=true` to pause the server's autonomous push; send `close=false` to resume.

### 7.2 Code skeleton (`esp-idf-svc`, structural reference)

Use `EspAsyncMqttClient`. Refer to the latest `esp-idf-svc` docs for API details.

**Connect + discovery subscription**:

```rust
use embedded_svc::mqtt::client::{AsyncClient, QoS};          // subscribe/unsubscribe/publish
use esp_idf_svc::mqtt::client::{EspAsyncMqttClient, MqttClientConfiguration};

let mut client = EspAsyncMqttClient::new(
    "mqtt://broker.example.com:1883",      // or mqtts://...:8883
    &mut MqttClientConfiguration {
        client_id: Some("vibetty-esp32-001"),   // must be unique within the broker
        username: Some("root"),                  // same broker account as vibetty
        password: Some("secret"),
        buffer_size: 32 * 1024,                  // ⚠️ must be large, see 7.3
        out_buffer_size: 8 * 1024,
        ..Default::default()
    },
)?;

// Discovery: subscribe to presence (retained → receive existing instances immediately)
client.subscribe("+/+/+/vibetty", QoS::AtLeastOnce).await?;
```

**Receive-message main loop**:

```rust
let mut current_prefix: Option<String> = None;
let mut current_format: Option<String> = None;   // the instance's `format` ("text" / "high" / ...)

loop {
    let msg = client.next().await?;
    let topic = msg.topic();
    let payload = msg.payload();
    let segs: Vec<&str> = topic.split('/').collect();

    match segs.as_slice() {
        // presence: [user, device, pid, "vibetty"] (4 segments)
        [_, _, _, "vibetty"] => {
            if payload.is_empty() {
                // LWT: instance offline → clear the current target
                current_prefix = None;
            } else {
                // {"prefix","client_id","ts","title","state","format"}
                let p: Presence = serde_json::from_slice(payload)?;
                if current_prefix.as_deref() != Some(&p.prefix) {
                    // unsubscribe the old instance's screen topic (whichever it was)
                    if let Some((old, fmt)) = current_prefix.take().zip(current_format.take()) {
                        let topic = if fmt == "text" { "screen_text" } else { "screen" };
                        client.unsubscribe(&format!("{old}/{topic}")).await?;
                    }
                    current_prefix = Some(p.prefix.clone());
                    current_format = Some(p.format.clone());
                    // subscribe the screen topic based on the instance's `format`
                    let topic = if p.format == "text" { "screen_text" } else { "screen" };
                    client.subscribe(&format!("{}/{topic}", p.prefix), QoS::AtLeastOnce).await?;
                    // report our own display size so the server resizes
                    client.publish(
                        &format!("{}/control", p.prefix),
                        QoS::AtLeastOnce, false,
                        br#"{"type":"sync","data":{"width":320,"height":240}}"#,
                    ).await?;
                }
            }
        }
        // JPEG screen image: [..., "vibetty", "screen"]
        [.., "vibetty", "screen"] => {
            // 1) detect JPEG by magic bytes → decode
            // 2) read the last 4 bytes = scrollback offset (0 = latest)
            // 3) display
        }
        // text screen stream: [..., "vibetty", "screen_text"]
        [.., "vibetty", "screen_text"] => {
            // payload[0] is the tag:
            //   0x00 = full-frame baseline → reset terminal buffer, replay payload[1..]
            //   0x01 = pty delta           → feed payload[1..] incrementally to the buffer
        }
        _ => {}
    }

    // Liveness fallback: in a separate timer, if the instance for current_prefix
    // hasn't updated ts in 30s → clear the target.
}
```

**Send input**:

```rust
// single key / raw bytes → pty_in
client.publish(&format!("{prefix}/pty_in"), QoS::AtLeastOnce, false, &[b'a']).await?;

// text command → control (JSON)
client.publish(
    &format!("{prefix}/control"),
    QoS::AtLeastOnce, false,
    br#"{"type":"input_text","data":"ls -la\n"}"#,
).await?;
```

### 7.3 Key details / pitfalls

1. **The buffer must be large**: `screen` is a full JPEG (can reach tens of KB); the ESP-IDF mqtt default `buffer_size` is too small and truncates it. Set `MqttClientConfiguration`'s `buffer_size` (receive) to at least 32KB, tuned to the actual screenshot size.
2. **Binary is fine**: `pty_in` / `screen` are raw bytes; esp-mqtt supports binary payloads. `control` is JSON (UTF-8).
3. **Handling retained correctly**: on first subscribing to presence / screen, the broker pushes all existing retained messages at once, so the remote has a complete online list + the most recent image right after connecting.
4. **LWT empty payload = delete**: receiving a presence message with an **empty** payload is the instance-offline signal; remove it from the list immediately.
5. **ts liveness fallback**: LWT only fires on abnormal disconnect; normal exit relies on `ts`. The remote tracks "last seen ts" and treats **`now - ts > 30s`** as offline (make sure the ESP32 clock is accurate, or use broker time).
6. **pid changes across restarts**: do not persist/cache the prefix; redo discovery on every startup.
7. **unsubscribe is asynchronous**: when it returns, the packet has only been sent; the broker may still deliver a few messages on that topic before the ACK — tolerate them (ignore unsubscribed topics).
8. **TLS**: use `mqtts://` for 8883; the ESP32 connects to TCP MQTT directly, not over WebSocket.
9. **Determining the `user` segment**: the remote's broker username **is** vibetty's `user` segment (provided vibetty set an account in the broker URL and the accounts match). If vibetty has no account (user segment = `root`), the remote uses the broad wildcard `+/+/+/vibetty`.
10. **ASR is done locally on the ESP32**: the server does no speech transcription. After the ESP32 recognizes speech, it just sends the **text** back via `control`'s `input_text`, saving audio bandwidth.
11. **Text mode (`format: "text"`)**: subscribe to `P/screen_text` (not `P/screen`) and dispatch on the leading tag byte (`0x00` full / `0x01` delta) — see §6.5b. Deltas are high-frequency and **not retained**, so the retained message is always a `0x00` baseline; if your terminal buffer gets out of sync, publish a `sync` to force a fresh `0x00`. The content contains ANSI escapes (needs a terminal parser, not a plain `print`). Use `sync.close=true` to mute the realtime deltas when not viewing.

### 7.4 Verify locally (without an ESP32)

First get the vibetty side running, then use the browser debug page / Python to simulate a remote and verify the protocol:

**① Start the broker + vibetty**

The easiest is to enable the built-in broker (`[mqtt] builtin_broker = true`) and run vibetty in the background:

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
```

(An external broker works too: `mosquitto -c /tmp/mosq.conf` with a minimal config `listener 1883 127.0.0.1` + `allow_anonymous true`.)

**② Browser debug page (recommended, zero code)**

Start the HTTP server (click the `HTTP` button in the TUI) and open `http://localhost:3000/mqtt_ws` in a browser: it connects to the built-in broker's WS port, discovers instances, shows the screen, and can send input.

**③ Python `paho-mqtt` to simulate a remote**

```bash
pip install paho-mqtt
```

```python
import paho.mqtt.client as mqtt, json, time

c = mqtt.Client()
c.connect("127.0.0.1", 1883)

def on_msg(_, __, m):
    print(m.topic, m.payload[:80])
c.on_message = on_msg

# Discovery (retained → receive existing instances immediately)
c.subscribe("+/+/+/vibetty")
c.loop_start(); time.sleep(2)

# Replace <prefix> with the value printed by on_msg above
prefix = "root/1a2b3c4d5e6f7a8b/12345/vibetty"
c.publish(f"{prefix}/pty_in", b"l")                                  # single key
c.publish(f"{prefix}/control", json.dumps({"type":"input_text","data":"ls\n"}))
time.sleep(2)
```

vibetty's terminal should show the output of `l` and the `ls` command.

**④ Test the LWT**: `tmux kill-session -t vibetty`, and check whether Python / mosquitto_sub receives an **empty payload** (presence deleted).

> Under the local sandbox, the `mosquitto_pub/sub` CLI sometimes reports "Bad file descriptor"; Python `paho-mqtt` is smoother.

---

## 8. The `skill` subcommand

Installs/removes the built-in `run-vibetty` SKILL.md into/out of an agent's **user-level** skills directory — after installing vibetty, one command sets it up, no manual folder copying. The skill content teaches an agent to "start a vibetty session in a background tmux and share the terminal screen to an ESP32 over MQTT".

```bash
vibetty skill install --claude          # → ~/.claude/skills/run-vibetty/
vibetty skill install --codex           # → ~/.agents/skills/run-vibetty/ (Codex USER scope)
vibetty skill install --claude --codex  # both
vibetty skill uninstall --claude        # remove (the directory is deleted only if it becomes empty)
```

- `--claude` / `--codex` are bool flags and can be given together; neither → error and exit.
- **Version-aware**: before install, it compares `CARGO_PKG_VERSION` with the companion file `.vibetty-version` in the target directory. Same version → skip; different version / no record → overwrite/upgrade. The version's single source of truth is `Cargo.toml`, so it follows releases automatically.
- **Safe uninstall**: deletes `SKILL.md` + `.vibetty-version`, and only removes the directory if it is then empty (**never** uses `remove_dir_all`, to avoid accidentally deleting `~/.claude/skills/` or `~/.agents/skills/`).
- The Codex path is `~/.agents/skills/` (not `~/.codex/`); see the USER scope at developers.openai.com/codex/skills.

---

## 9. Debugging and FAQ

**Where are the logs?**
vibetty uses flexi_logger to write logs to the **CWD** (the directory it was started in, or the directory passed via tmux `-c`), in files named `vibetty*.log`, in append mode with 10MB rotation. **Nothing goes to stdout.** Look there when troubleshooting.

**Is it a problem if I haven't configured `[mqtt]`?**
No. With no `[mqtt]` section in `~/.vibetty/config.toml`, startup should produce **zero MQTT logs**, and TUI / PTY behavior is unchanged.

**It exits immediately after startup / `tmux ls` reports no server running?**
Most likely you **didn't pass `-- <command>`**. The PTY has no program to run, hits EOF immediately, and vibetty exits; the tmux session disappears within seconds. This is easily mistaken for a port conflict or a startup failure.

**Why can't I access HTTP?**
The HTTP server is **off by default**; you must start it via the `HTTP` button in the TUI. Without starting it, there is no `/screenshot` or `/mqtt_ws`.

**The ESP32 receives no screen?**
Check in order:
1. Are the ESP32 and vibetty on **the same broker** (consistent host/port/credentials)?
2. Is the ESP32's `buffer_size` large enough (≥ 32KB)? The default truncates `screen`.
3. Is the discovery wildcard correct? When vibetty has no broker account, the user segment is `root` — use `+/+/+/vibetty`.
4. Did vibetty's PTY produce output to trigger a broadcast? In headless mode, `screen` is only sent when the PTY has output.
5. The built-in broker listens on `0.0.0.0`; confirm the ESP32's network can reach the PC's port 1883 (firewall, etc.).

**Can the built-in broker be stopped?**
No. rumqttd has no shutdown API; `Start broker` can only start, not stop. Only stopping the entire vibetty process takes it down.

**Changing the `qos` field has no effect?**
Correct. `[mqtt] qos` is a reserved field and not currently effective; inbound QoS is hardcoded (`pty_in=0`, `control=1`).

**Does it run headless / without a TTY?**
Yes; the default 80×24 size is already handled (no panic). But receiving `screen` still requires the PTY to have output to trigger a broadcast. Running it in a background tmux is the most reliable approach.

---

## 10. Relevant source files

| File | Purpose |
|---|---|
| `src/mqtt.rs` | MQTT bridging: topic construction (`instance_prefix`), presence (`presence_payload`), LWT, heartbeat, `parse_control`, `INBOUND_TOPICS`, `screen` rendering/publishing. **The protocol is defined by this file.** |
| `src/config.rs` | `Cli` / `RunArgs` / `MqttConfig` (all `[mqtt]` fields), `MqttConfig::for_client()` (URL parsing, the single source), `mqtt_config()` (read config). |
| `src/protocol.rs` | `ClientMessage` (the serde source for `control` JSON), `ServerMessage`, `ImageFormat`. |
| `src/terminal/agent.rs` | `AgentState` (Working/Waiting); detects Codex / Claude Code state from the terminal title. |
| `src/ws.rs` | `run_command` (main loop: PTY, TUI, buttons, boot auto-start MQTT, agent state broadcast), `SCREEN_*` rendering constants, `render_screen_to_image`, `/screenshot` + `/mqtt_ws` routes. |
| `src/broker.rs` | `spawn_builtin()`: runs rumqttd on a dedicated thread (TCP + WS, anonymous, 1MB payload). No shutdown. |
| `src/setup.rs` | The `vibetty setup` ratatui TUI; edits all `[mqtt]` fields and writes them back to the config. |
| `src/skill.rs` | `vibetty skill install/uninstall` implementation; embeds `resources/skills/run-vibetty/SKILL.md`. |
