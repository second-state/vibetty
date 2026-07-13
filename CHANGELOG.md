# Changelog

## [0.4.0-rc.3] - 2026-07-14

Screen traffic optimization + JPEG quality tiers. The screen is sent less aggressively now, and you can trade image size for quality via a new flag. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Added

- **JPEG quality tiers (`-q, --quality`)**: screen output is always JPEG; pick `high` (q85 color, default), `medium` (q70 color), or `low` (q50 grayscale). Replaces the old `-f` image-format flag.
- **Screen byte counter**: the MQTT outbound task now logs cumulative screen bytes (MB) on each publish, for traffic debugging.

### Changed

- **Less screen traffic**: the screen is no longer broadcast on every PTY output. On startup it waits for the PTY to settle (500ms quiet, 3s cap) before sending the first frame + presence; during a run, a single large PTY output (>512 bytes) triggers a 500ms debounce that coalesces bursts and sends the latest frame once quiet, while small outputs still send immediately.

## [0.4.0-rc.2] - 2026-07-13

A small follow-up to rc.1: a new `skill` subcommand, an MQTT reconnect fix, and quieter logs. The docs are also overhauled. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Added

- **`vibetty skill` subcommand**: install / uninstall the bundled `run-vibetty` SKILL.md into the Claude Code and/or Codex user-level skills directories. Version-aware (skips when the same version is already installed, upgrades on mismatch) and uninstall-safe (never `remove_dir_all`; only deletes the directory if it becomes empty).
- **Bilingual usage guide**: a detailed `docs/USAGE.md` (English) and `docs/USAGE.zh.md` (中文), covering installation, configuration, the TUI, HTTP endpoints, the full MQTT protocol, and an ESP32 / MCU integration guide.

### Fixed

- **MQTT reconnect**: inbound topics (`pty_in` / `control`) are now re-subscribed on every ConnAck, and presence is re-published on reconnect. Previously a reconnect could leave the instance "connected but receiving nothing" until the next 15s heartbeat, and its presence stayed cleared until then.
- **Dropped the dead `PtyOutput` broadcast path**: with the browser front-end removed, `PtyOutput` had no consumer, so the leftover broadcast was removed.

### Changed

- **Quieter logs**: high-frequency WebSocket event logs were demoted from `info` to `debug`.
- **Larger internal channels**: broadcast / mpsc channel capacities were raised 100 → 1024 to avoid drops under load.
- **Docs overhaul**: rewrote the README for the current feature set (dropped the removed ASR / voice content); removed the standalone `docs/esp32-mqtt-integration.md` (its protocol content now lives in the usage guide).

---

## [0.4.0-rc.1] - 2026-07-13

The first release to introduce the MQTT transport. Alongside the existing WebSocket (`/ws`) channel, an optional MQTT channel is added so that devices that cannot easily run a WebSocket client (e.g. ESP32 / MCUs) can connect. Both channels coexist and are driven by the same PTY session. MQTT is only enabled when the config file contains a `[mqtt]` section; otherwise behavior is unchanged. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Added

- **MQTT transport channel**: a second transport channel alongside WebSocket, sharing the same PTY session, `cli_tx`, and broadcast `tx`. Enabled only when a `[mqtt]` section is present in the config; otherwise MQTT is never touched.
- **MQTT protocol split**: inbound keystrokes go to a dedicated raw topic `pty_in`; control messages (text input / sync / scroll) are merged into `control` (payload is `ClientMessage` JSON); the outbound side publishes a single full `screen` image (no chunking, format distinguished by magic bytes).
- **Built-in rumqttd broker**: a local broker (TCP + WS, anonymous, 1 MB payload) can be auto-started on boot, or you can point the client at a self-hosted broker or a free cloud service instead.
- **MQTT presence announcement**: an online status is announced (retained) on the instance topic with a 15-second heartbeat; on abnormal disconnect the LWT (empty retained) clears it automatically — no manual teardown needed.
- **Multi-instance discovery**: presence topics are prefixed `{user}/{device}/{pid}/vibetty`; an ESP32 can subscribe to `{user}/+/+/vibetty` to discover every instance under that user.
- **Terminal agent state tracking**: the terminal window title is parsed to detect the working / waiting state of Codex and Claude Code.
- **Agent state broadcast over MQTT**: the working / waiting state is published with presence, so an ESP32 can decide whether it needs to push the screen to the user.
- **TUI MqttPanel**: clicking the `MQTT` button opens a panel where you can start / stop the client, start the built-in broker, and edit the broker URL and ports (Enter writes back to the config).
- **TUI top button row**: the HTTP / MQTT / Fit / Quit buttons move from the bottom to the first screen row; the MQTT button text reflects the combined state (`off` / `brkr` / `conn` / `on`).
- **TUI hover highlight**: buttons highlight on mouse hover, using any-event mouse reporting with two layers of throttling to avoid redraw storms.
- **Fit button**: one click resets the terminal size to the current window size (the usable area after subtracting the button row and the terminal's top border).
- **On-demand HTTP server**: start / stop the HTTP server via a button instead of keeping it always on.
- **Quit button**: exit directly from the TUI.
- **`vibetty setup` config TUI**: `vibetty setup` is now a ratatui interface that edits every field of `[mqtt]` and writes it back to the config file (other sections preserved).
- **`--config` flag**: a new `--config` flag overrides the default `~/.vibetty/config.toml` path.

### Changed

- **Transport**: WebSocket (`/ws`) remains the default; MQTT is an optional second channel that coexists with it, both driven by the same PTY session.
- **ASR moved to ESP32**: speech recognition now runs locally on the ESP32; the recognized text is sent back over `control`. The server no longer performs transcription or audio processing.
- **Auto-scroll on waiting**: when the terminal agent switches to the waiting state, the screen scroll is automatically reset to the latest (scrollback = 0).
- **Default screenshot format**: terminal screenshots now default to JPEG (configurable).
- **Broker URL is config-driven**: the broker URL the client connects to is always taken from the configured `broker`; it only falls back to the local address when `broker` is empty and the built-in broker is enabled.

### Removed

- **Browser front-end**: removed the `app.js`, `index.html`, `setup.html`, and `vosk/` resources under `resources/`.
- **Server-side ASR**: removed Whisper HTTP and Alibaba Cloud Bailian realtime WS transcription, WAV / PCM audio processing, and the related dependencies (`wav_io`, `reqwest`, `reqwest-websocket`, `hanconv`).
- **change-directory feature**: removed the old working-directory switching feature.
- **Legacy WebSocket terminal front-end**: removed the superseded legacy terminal front-end.

---

## Background: why MQTT was added

vibetty now offers MQTT as a second transport channel alongside WebSocket.

The existing WebSocket approach has an obvious limitation: vibetty runs on your PC, while vibekeys needs to reach it from an external network, which forces you to expose vibetty's port to the public internet — either via a port-mapping service or by setting up a separate forwarding server. That is cumbersome and adds cost and risk.

With MQTT, both vibetty and vibekeys connect as clients to the same MQTT broker, so the port on your PC no longer has to be exposed:

- **Self-host the broker**: deploy it on your own server or device, so none of the traffic ever touches a third party.
- **Use a free MQTT cloud service**: if you don't want to self-host, registering a free MQTT cloud service (EMQX Cloud) is enough, and fully sufficient for a single-user scenario.
- **Share a broker with trusted people**: the same broker can be shared with trusted family, friends, or colleagues, with no need for each person to build or buy their own — further reducing cost.

Whichever you choose, your data stays in your own hands — safer by design.

---

For deployment details or any questions, feel free to reach out.
