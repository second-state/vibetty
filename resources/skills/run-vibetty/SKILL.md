---
name: run-vibetty
description: Start vibetty in a background tmux session to share an interactive terminal session (claude / shell / any CLI) over MQTT in real time to remote devices (ESP32 / MCU, etc.). Covers prerequisites, the start command, verification, and common pitfalls (a PTY command must be passed; [mqtt] must be configured first).
---

# Share a terminal session over MQTT

vibetty can share an interactive terminal session (claude, a shell, or any CLI) over MQTT in real time: the terminal screen on your PC is published as screenshots to an MQTT topic, and remote devices (ESP32 / MCU, etc.) can subscribe to display that session live and send keystrokes back.

This skill starts such a sharing session inside a background tmux session, so it keeps running without occupying your current terminal.

## Prerequisite: configure MQTT

Before first use, configure the MQTT broker. Run the following to open the config UI (a ratatui TUI) and fill in the `[mqtt]` fields (broker address, port, whether to auto-start the built-in broker, etc.):

```bash
vibetty setup
```

Or manually add a `[mqtt]` section to `~/.vibetty/config.toml`.

> vibetty enables the MQTT transport only when a `[mqtt]` section is present in the config; otherwise it uses WebSocket only, publishes no MQTT messages, and never reaches the broker.

The broker can be self-hosted (rumqttd / mosquitto / EMQX, etc.) or a free MQTT cloud service (e.g. EMQX Cloud).

## Start a sharing session

```bash
tmux new-session -d -s vibetty -c "<working_dir>" 'vibetty -- <command_to_run_in_the_terminal>'
```

Example (working directory `~/workspace`, running `claude` in the terminal):

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
```

- `-s vibetty`: the tmux session name; customize as you like. To share multiple sessions at once, give each a distinct name.
- `-c "<working_dir>"`: the working directory for the terminal session. **Use `-c`; do not use `cd ... && tmux`** (the latter does not switch the directory as expected when a tmux server is already running).
- `-- <command>`: the program to run inside the shared terminal. `claude`, `bash -l`, or any CLI works.

## Verify the session started

```bash
sleep 6
tmux ls
tmux capture-pane -t vibetty -p | tail -20
```

- `tmux ls` lists a session named `vibetty`.
- `capture-pane` shows that the terminal program (e.g. claude) is up.
- The `MQTT` button at the top of vibetty shows `conn`, meaning it has connected to the broker and is sharing.

## Common pitfalls

- **You must pass `-- <command>`**. If omitted (just `vibetty`), the PTY has no program to run and immediately hits EOF; vibetty then exits gracefully, and the tmux session disappears within seconds — `tmux ls` reports `no server running`. This is easily mistaken for a port conflict or a startup failure; the actual cause is that the shared terminal is empty and exits at once.
- **Configure `[mqtt]` first**. Without a `[mqtt]` section, vibetty does not enable MQTT and remote devices receive nothing; run `vibetty setup` to configure the broker before starting.
- **The HTTP server is off by default**. HTTP is for the browser front-end; when sharing only over MQTT you don't need it — toggle the `HTTP` button on demand.
- **Log location**: vibetty writes logs to its CWD (the directory passed via `-c`), in files named `vibetty*.log` (append mode, with rotation). Check there when troubleshooting.
- **Binary install**: install to `~/.cargo/bin/vibetty` with `cargo install`, or download a prebuilt binary for your platform from the GitHub Release.

## Manage the session

- `tmux attach -t vibetty` — enter the session to view or operate the shared terminal.
- `Ctrl-b d` — detach; the session keeps running in the background and keeps sharing.
- `tmux kill-session -t vibetty` — end the session and stop sharing.
