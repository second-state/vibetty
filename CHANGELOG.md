# Changelog

## [0.4.0] - 2026-07-29

The first stable MQTT release. All rc.1–rc.9 changes rolled up. Summary of what 0.4.0 delivers vs 0.3.x:

### Added

- **MQTT transport** as the primary sharing channel — share a PTY session over MQTT (raw TCP or WebSocket), no port exposure needed. Both ends are just MQTT clients to the same broker.
- **Text output mode (`-q text`, the new default)** — the screen is sent as an ANSI terminal stream on `{p}/screen_text` (tag `0x00` = full-frame baseline, `0x01` = realtime PTY delta). Clients with a terminal emulator render directly. JPEG mode (`-q high/medium/low`) remains available.
- **Built-in broker** — an in-process rumqttd broker (TCP + WS) can auto-start, zero external infrastructure.
- **Multi-instance discovery** — each instance announces itself with retained presence; clients discover all instances via a single wildcard subscription.
- **Agent state detection** — terminal title parsed for Codex / Claude Code working/waiting state, broadcast in presence.
- **Web debug page** (`/mqtt_ws`) — full-featured MQTT-over-WebSocket client (DaisyUI UI, xterm.js for text mode, keyboard passthrough, session list, mobile layout). Self-contained HTML, deployable standalone.
- **`Sync.pixels` field** — clients can report display size in pixels (default) or character cols/rows (`pixels=false`).
- **`Sync.close` field** — clients can pause/resume the server's autonomous screen push to save bandwidth.
- **Presence `format` field** — advertises the output mode (high/medium/low/text) so clients know which screen topic to subscribe to.
- **`vibetty skill` subcommand** — install/uninstall the bundled `run-vibetty` SKILL.md into Claude Code / Codex user-level skills directories.

### Changed

- **`-q` default is now `text`** (was `high`). The `-q` flag now selects output format: `text` / `high` / `medium` / `low`.
- **Screen topics are not retained** — `{p}/screen` and `{p}/screen_text` (full + delta) are all `retain=false`; only presence is retained. Prevents stale retained messages from accumulating on the broker after restarts (topic prefix includes pid).
- **No clean MQTT DISCONNECT on stop** — the connection is dropped, so the broker always fires the LWT to clear presence.
- **biased select!** in both the MQTT bridge and the main event loop — inbound control (sync/pty_in/close) is prioritized over PTY output, so control stays responsive during heavy output bursts.
- **Resize burst absorption** — after a PTY resize (which triggers a TUI redraw burst), output is absorbed for up to 500ms and a single full frame is sent once output settles.
- **Simpler screen debounce** — every PTY output activates a 100ms trailing timer; only after 100ms of quiet is the latest frame sent.
- **Scroll paging keeps 2 rows** of overlap (was 1).
- **`JpegQuality` renamed to `OutputFormat`** — now covers the `text` mode too.

### Fixed

- **Backspace key** sends DEL (`0x7f`), not BS (`0x08` = Ctrl+H).
- **Navigation-key modifiers** (Shift/Ctrl/Alt + arrows/Home/End/PgUp/PgDn/Delete/Insert) are now properly encoded per the xterm "Modified Keys" spec.

---

## [0.4.0-rc.9] - 2026-07-27

Keep MQTT control responsive under heavy output. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Fixed

- **Control messages no longer starve during output bursts.** Both `select!` loops (the MQTT bridge and the main event loop) are now `biased` and rank inbound control (`sync` / `pty_in` / `close`) ahead of PTY output. Previously, heavy outbound publishing to a slow broker could starve the inbound poll, so even a `close=true` (the very message meant to mute the flood) couldn't get through.
- **`sync` with `close=false` sends a screen frame immediately again.** The resize-settle change had made every sync wait 500ms; now the settle only triggers when the sync actually resized the PTY, and a non-closing sync replies at once.

### Changed

- **Less per-output work in text mode.** The redraw closure borrows `&Screen` instead of `&Arc<Screen>`, and the PtyOutput handler no longer clones the whole screen grid up front (text mode broadcasts raw bytes). The full-screen clone now happens only on the JPEG debounce path.

## [0.4.0-rc.8] - 2026-07-26

Text-mode QoS tuning + resize-burst handling. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Changed

- **`screen_text` QoS split**: the full-frame baseline (tag `0x00`) is now published at QoS 1 (low-frequency, worth delivering reliably); the realtime pty delta (tag `0x01`) stays at QoS 0 (high-frequency, a missed frame is harmless). JPEG `screen` and `pty_in` remain QoS 0; presence is QoS 1.
- **Resize no longer floods deltas.** Resizing the PTY (sync / window resize / Fit) triggers a TUI redraw burst. vibetty now absorbs that burst for up to 500ms (resetting on each new chunk) and sends a single full screen frame once output stays quiet, instead of forwarding every intermediate chunk as a pty_out delta. Normal output is unchanged.

## [0.4.0-rc.7] - 2026-07-26

Default output mode + retained-message hygiene. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Changed

- **`-q text` is now the default** (was `high`). Without `-q`, vibetty now sends the screen as an ANSI text stream on `P/screen_text` instead of a JPEG image.
- **Screen topics are no longer retained.** `{p}/screen` and `{p}/screen_text` (both full-frame `0x00` and delta `0x01`) are published with `retain=false`; only presence stays retained. The topic prefix contains the pid, which changes every restart, so retained screen frames used to pile up on stale `{old-pid}/...` topics that nobody cleared. The remote now gets its first frame by sending a `sync` on connect.
- **No clean MQTT DISCONNECT on stop.** vibetty no longer calls `client.disconnect()`, so the broker always sees the dropped socket as an abrupt disconnect and fires the LWT to clear the retained presence (a clean DISCONNECT would suppress the LWT and leak the presence on the old pid's topic).

## [0.4.0-rc.6] - 2026-07-20

Text output mode + MQTT protocol expansion (raw PTY stream, richer Sync, format discovery). (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Added

- **Text output mode (`-q text`)**: alongside the JPEG quality tiers, the screen can now be sent as an ANSI terminal stream on a new `{p}/screen_text` topic, instead of a JPEG image. Every payload starts with a 1-byte tag: `0x00` = full-frame baseline (`vt100` `contents_formatted`, replayable ANSI with colors) and `0x01` = realtime raw PTY delta. Full frames are retained, deltas are not (so a reconnect always receives a complete baseline). `/screenshot` returns `text/plain` in this mode.
- **Realtime PTY stream in text mode**: PTY output is published immediately as `0x01` deltas on `{p}/screen_text` (the separate `{p}/pty_out` topic is gone — deltas merged into `screen_text`). JPEG mode is unchanged (debounced `{p}/screen` frames).
- **`Sync.pixels` field**: when `false`, the client sends character cols/rows directly instead of pixels (server skips the pixel→cell conversion). Defaults to `true` (pixels) for backward compatibility.
- **`Sync.close` field**: a pause switch for the server's autonomous screen push. `close=true` stops PTY-output-triggered publishing (and drops the in-flight debounced frame); `close=false` resumes. Defaults to `false`. Client-initiated responses (sync reply, scroll) are unaffected. Lets power-constrained clients mute the stream when not viewing.
- **`format` in presence**: the instance's `-q` setting (`high`/`medium`/`low`/`text`) is now advertised in the presence JSON, so clients know whether to subscribe to `{p}/screen` (JPEG) or `{p}/screen_text` (text).

### Changed

- **`JpegQuality` → `OutputFormat`**: the enum was renamed now that it covers a non-JPEG (text) mode, and gained `Text` / `is_text()` / `as_str()`. Wire format unchanged for the existing tiers (`high`/`medium`/`low`); `text` added.
- **Rendering decision moved to the MQTT bridge**: `ws` only broadcasts the `Screen`; the MQTT task renders it as JPEG or ANSI text based on `image_format`. `ScreenText` is no longer a separate protocol variant.
- **Text-mode traffic now counted**: the MQTT outbound byte counter (`total_screen_bytes`) now accumulates text-mode full frames and deltas too (previously JPEG only); both log the running total in MB.

## [0.4.0-rc.5] - 2026-07-15

Keyboard input fixes for full-screen editors (helix / vim / zerostack / …). (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Fixed

- **Backspace sent Ctrl+H**: the Backspace key was encoded as `0x08` (BS = Ctrl+H) instead of `0x7f` (DEL), so editors that key off Backspace received Ctrl+H instead. Now sends DEL.
- **Navigation-key modifiers were dropped**: Shift/Ctrl/Alt held together with arrows, Home, End, PgUp, PgDn, Delete, or Insert were ignored and a plain sequence was sent, so editor shortcuts (Shift+Arrow to select, Ctrl+Left/Right to jump by word, Ctrl+Delete, …) did not work. Modifiers are now encoded per the xterm "Modified Keys" spec (e.g. Ctrl+Right → `\x1b[1;5C`, Shift+Up → `\x1b[1;2A`, Ctrl+Delete → `\x1b[3;5~`). Plain (unmodified) keys are unchanged.

## [0.4.0-rc.4] - 2026-07-15

Codex status detection + simpler screen debounce + scroll context. (中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md).)

### Added

- **Better Codex status detection**: Codex's working/waiting state is now read from the braille spinner in the terminal title (⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏) — spinner present means *working* — so the presence state flips more reliably than the previous title-prefix matching.

### Changed

- **Simpler screen debounce**: every PTY output now starts (or refreshes) a 100ms timer and sends the latest frame once output has been quiet for 100ms, coalescing a burst into a single screen. This replaces the rc.3 conditional debounce (which only debounced outputs over 512 bytes; small outputs were sent immediately).
- **Scroll keeps 2 rows**: paging up or down now keeps 2 rows of overlap with the previous view (was 1) for more context.

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
