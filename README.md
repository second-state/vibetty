# vibetty

Share an interactive terminal session (`claude`, `codex`) running on your PC with remote devices (ESP32, MCU, another machine) in real time — over MQTT.

vibetty runs a program in a PTY and publishes the terminal screen as ANSI text frames over MQTT. Remote clients subscribe to display the live screen and send keystrokes back. Both ends are just MQTT clients talking to the same broker, so **no port on your PC needs to be exposed** to the public internet.

> MQTT is the primary sharing transport. It is opt-in: it turns on only when a `[mqtt]` section exists in the config; otherwise MQTT is never touched. See the [changelog](CHANGELOG.md).
>
> **⚠️ Firmware update required:** vibetty `0.4.x` changed the MQTT protocol (new topics, `screen_text` tag bytes, `Sync.pixels`/`close` fields, presence `format`). If you use a **vibekeys** keyboard device, **update its firmware to `v0.4.0+`** — older firmware will not be able to connect or display correctly.

## Features

- **Live terminal sharing over MQTT** — the screen is published as ANSI text frames (full baseline + raw PTY increments); remote devices subscribe and send keystrokes back.
- **Built-in broker** — pointing the broker URL at localhost makes vibetty auto-start an in-process rumqttd broker (TCP; optional WS for the debug page), so you need zero external infrastructure.
- **Multi-instance discovery** — each instance announces itself with a retained presence; a client discovers every one of your instances with a single wildcard subscription.
- **Agent state detection** — the terminal title is parsed to tell whether Codex / Claude Code is `working` or `waiting`, and that state is broadcast alongside presence.
- **TUI controls** — the terminal fills the window; a menu popup (agent status / MQTT / Fit / Quit) is toggled with a hotkey (`ctrl+shift+p` by default, configurable). The MQTT entry reflects the combined state (`off` / `brkr` / `conn` / `on`).
- **Cross-platform** — Linux, macOS, Windows (via ConPTY).

## Quick start

### 1. Install

One-liner (downloads the latest prebuilt binary to `~/.cargo/bin`):

```bash
curl -fsSL https://raw.githubusercontent.com/second-state/vibetty/main/install.sh | bash
```

**Upgrading**: just re-run the same command after a new release — the script installs the new version as `vibetty-<version>` and re-points the `vibetty` symlink, so running instances are never disturbed. If the latest version is already installed it says so and exits. To install a specific (pre)release instead, pass the tag: `... | bash -s -- v0.4.3-rc.1`. Old versions accumulate in `~/.cargo/bin`; the script offers to delete them after an upgrade.

Or download a prebuilt binary for your platform from the [Releases page](https://github.com/second-state/vibetty/releases) and put it on your `PATH` (e.g. `~/.cargo/bin`).

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

This opens a TUI to fill in the `[mqtt]` fields and writes `~/.vibetty/config.toml`. The simplest setup is the built-in broker — just point the broker at localhost; vibetty starts its built-in broker on that port automatically, no external service needed:

```toml
[mqtt]
broker = "mqtt://127.0.0.1:1883"
```

Or point at your own broker / a free MQTT cloud:

```toml
[mqtt]
broker = "mqtt://user:pass@broker.example.com:1883"   # mqtts:// for TLS
```

> Without a `[mqtt]` section, vibetty does not enable MQTT and never reaches the broker.

Optional UI settings: the menu popup (MQTT / Fit / Quit + agent status) is toggled with a
hotkey, `ctrl+shift+p` by default. Override it in the same config file:

```toml
[ui]
menu_key = "ctrl+g"   # modifiers: ctrl / shift / alt / super + a single character key
border = false        # draw a ratatui border around the terminal (toggled from the menu too);
                      # the border color follows the agent state and the top-right corner
                      # shows the current status
```

### 3. Run

```bash
vibetty -- claude        # share a `claude` session
vibetty -- codex         # share a `codex` session
```

You **must** pass `-- <command>`. The MQTT entry in the menu popup (`<menu_key>` / `ctrl+shift+p`) shows `conn` once it has connected and is sharing.

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

| Flag                      | Description                                                                                                | Default                  |
| ------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------ |
| `-- <command>`            | Program to run in the PTY (e.g. `-- claude`)                                                               | _(required)_             |
| `--config <PATH>`         | Override the config file path                                                                              | `~/.vibetty/config.toml` |
| `-a, --auto-submit`       | Append Enter to `input_text` so a sent command is executed                                                 | `true`                   |

## Browser debug page (served by you)

`resources/mqtt_ws.html` is a self-contained MQTT-over-WebSocket viewer: it connects to the built-in
broker's WS port (`builtin_ws_port` in the config; `0` disables it), discovers instances, shows the
screen, and can send input. vibetty no longer ships an HTTP server — host the file with any static
server (e.g. `python3 -m http.server`) and open it in a browser. Handy for testing without hardware.

## Use as a Herdr plugin

vibetty ships a Herdr plugin manifest. Install it into Herdr with one command —
this fetches the repo, builds vibetty, and registers the plugin (the `vibetty`
binary ends up on your PATH, since Herdr resolves pane commands through PATH):

```bash
herdr plugin install second-state/vibetty
```

For local development, clone the repo and link it instead:

```bash
git clone https://github.com/second-state/vibetty && cd vibetty
herdr plugin link .
```

Then configure MQTT once (`vibetty setup`), and trigger the `share` action from
the Herdr command palette — or bind a key (below).

### Bind a hotkey

Add this to `~/.config/herdr/config.toml` (`key = ""` leaves it unset — pick your
own, e.g. `prefix+i`):

```toml
[[keys.command]]
key = "prefix+i"
type = "plugin_action"
command = "vibetty.share"
description = "Share this agent pane over Vibetty"
```

Then reload Herdr's config (`herdr server reload-config`, or restart). Pressing
the key in any agent pane opens the vibetty status bar, which shows
`<agent> ▸ <pane> · [MQTT · <X.XX MB>] · <title>` (`[MQTT ...]` turns green once
connected). Press `q` (or `Ctrl+C`) in that pane to stop sharing.

**MQTT not connecting?** The `[MQTT ...]` bracket stays gray until vibetty
connects. If it never turns green, run `vibetty setup` to set the broker address
(writes `~/.vibetty/config.toml`), then trigger `share` again.

## Documentation

- **[Detailed usage guide](docs/USAGE.md)** — configuration, the TUI, the full MQTT protocol, and an ESP32 / MCU integration guide. ([中文版](docs/USAGE.zh.md))
- [Changelog](CHANGELOG.md) · [中文更新日志](docs/CHANGELOG.zh-CN.md)

## Platform support

Linux, macOS, and Windows (via ConPTY).
