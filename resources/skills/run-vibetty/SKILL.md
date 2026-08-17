---
name: run-vibetty
description: Share an interactive terminal session (claude / shell / any CLI) over MQTT in real time to remote devices (ESP32 / MCU, etc.). Two routes: inside Herdr, trigger the vibetty plugin share action on the focused agent pane; otherwise start vibetty in a background tmux session. Covers prerequisites, the start commands, verification, and common pitfalls (a PTY command must be passed; [mqtt] must be configured first).
---

# Share a terminal session over MQTT

vibetty can share an interactive terminal session (claude, a shell, or any CLI) over MQTT in real time: the terminal screen on your PC is published to an MQTT topic, and remote devices (ESP32 / MCU, etc.) can subscribe to display that session live and send keystrokes back.

Pick the route that matches where you are running:

- **Inside Herdr** (`test "${HERDR_ENV:-}" = 1` passes): use the vibetty Herdr plugin — see the next section.
- **Anywhere else**: start a background tmux session — see "Start a sharing session (tmux)" below.

Herdr control commands return JSON. Prefer `jq` to extract IDs when it is installed (e.g. `... | jq -r '.result.root_pane.pane_id'`); it fails cleanly on non-JSON output, unlike inline `python3 -c "json.load(...)"` which throws tracebacks on error responses.

## Prerequisite: configure MQTT (both routes)

Before first use, configure the MQTT broker. Run the following to open the config UI (a ratatui TUI) and fill in the `[mqtt]` fields (broker address, port, whether to auto-start the built-in broker, etc.):

```bash
vibetty setup
```

Or manually add a `[mqtt]` section to `~/.vibetty/config.toml`.

> vibetty enables the MQTT transport only when a `[mqtt]` section is present in the config; otherwise it publishes no MQTT messages and never reaches the broker.

The broker can be self-hosted (rumqttd / mosquitto / EMQX, etc.) or a free MQTT cloud service (e.g. EMQX Cloud).

## Route 1: inside Herdr — share the focused agent pane

Requires the vibetty binary on PATH and the vibetty Herdr plugin. Install once:

```bash
# 1) Binary: prefer the install script (prebuilt, fast). If it fails (unsupported
#    platform, no network to GitHub releases, ...), fall back to building:
curl -fsSL https://raw.githubusercontent.com/second-state/vibetty/main/install.sh | bash \
  || cargo install --git https://github.com/second-state/vibetty

# 2) Plugin: register with Herdr (fetches the repo, builds via manifest if the
#    binary is not already on PATH, registers the share action):
herdr plugin install second-state/vibetty
```

Start sharing the focused pane (the pane this command runs in):

```bash
herdr plugin action invoke share --plugin vibetty
```

This opens a 1-row vibetty status pane below the focused pane; the focused pane's terminal is now shared over MQTT. The status pane auto-closes when vibetty exits.

Verify:

```bash
herdr pane list --workspace "$HERDR_WORKSPACE_ID"
```

A new pane appears below the calling pane. Visually it shows `<agent> ▸ <pane> · [MQTT · <X.XX MB>] · <title>`; the `[MQTT ...]` bracket turns green once connected. If it stays gray, MQTT is not configured — run `vibetty setup`, then invoke the share action again (close the old status pane first).

Stop sharing: focus the vibetty status pane and press `q` (or Ctrl+C), or close it directly:

```bash
herdr pane close <vibetty-pane-id>
```

## Route 2: start a sharing session (tmux or Herdr)

This route starts a **new** terminal running your chosen program (e.g. `claude`) with vibetty sharing it, without occupying your current terminal. Use it when you want to share a fresh session rather than an existing pane.

### With tmux (outside Herdr)

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

### With Herdr: start an agent in a new tab, then share it

Inside Herdr (`HERDR_ENV=1`), the equivalent is: create a **new tab** in the current workspace with the wanted cwd, start the agent in its root pane, then trigger the share action on that pane.

1. Create a tab in the current workspace with the target working directory (keep the user's focus unchanged):

   ```bash
   herdr tab create --workspace "$HERDR_WORKSPACE_ID" --cwd "<working_dir>" --no-focus
   ```

   Read the root pane ID from `.result.root_pane.pane_id` (and the tab ID from `.result.tab.tab_id`).

2. Start the agent in that root pane (it must be an available shell pane; the name must be unique, `[a-z][a-z0-9_-]{0,31}`):

   ```bash
   herdr agent start <name> --kind claude --pane <root-pane-id>
   ```

   `agent start` returns only after Herdr detects the agent and it is ready for input. Pass native agent arguments only after `--`.

3. Share that pane — focus its tab (the share action targets the focused pane; `herdr pane focus` only accepts directions, not IDs), then trigger the vibetty share action:

   ```bash
   herdr tab focus <tab-id>
   herdr plugin action invoke share --plugin vibetty
   ```

   This opens the 1-row vibetty status pane below it and starts sharing (see Route 1). Submit work through the agent surface while it runs:

   ```bash
   herdr agent prompt <name> "<task>" --wait --timeout 120000
   herdr agent read <name> --source recent-unwrapped --lines 120
   ```

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
- **Log location**: vibetty writes logs to `~/.vibetty/logs/`, one file per working directory (`vibetty-<cwd-with-slashes-as-dashes>_rCURRENT.log`, append mode, with rotation). Check there when troubleshooting (both routes).
- **Binary install**: `curl -fsSL https://raw.githubusercontent.com/second-state/vibetty/main/install.sh | bash` (prebuilt, installs `vibetty-<version>` plus a `vibetty` symlink into `~/.cargo/bin`), `cargo install`, or a prebuilt binary from the GitHub Release.

## Manage the session

- `tmux attach -t vibetty` — enter the session to view or operate the shared terminal.
- `Ctrl-b d` — detach; the session keeps running in the background and keeps sharing.
- `tmux kill-session -t vibetty` — end the session and stop sharing.
