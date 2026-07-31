# vibetty

把运行在你 PC 上的交互式终端会话（`claude`、`codex`）通过 MQTT **实时**分享给远端设备（ESP32、MCU、另一台机器）。

vibetty 在一个 PTY 里跑程序，把终端画面渲染成图片后发布到 MQTT。远端客户端订阅它来显示实时画面，并把按键发回来。两端都只是连同一个 broker 的 MQTT 客户端，**你的 PC 不需要对外暴露任何端口**。

> 0.4.0 引入 MQTT 作为主要的分享通道，另保留一条可选的 HTTP 通道，两者共用同一个 PTY 会话。MQTT 是可选的：只有配置里存在 `[mqtt]` 段时才启用，否则完全不碰 MQTT。详见[更新日志](../CHANGELOG.md)。
>
> **⚠️ 需要更新固件：** vibetty `0.4.x` 改了 MQTT 协议（新 topic、`screen_text` tag 字节、`Sync.pixels`/`close` 字段、presence `format`）。如果你使用 **vibekeys** 键盘设备，**请将固件更新到 `v0.4.0+`** —— 旧固件无法正常连接或显示。

## 功能特性

- **MQTT 实时分享终端** —— 把屏幕作为 JPEG 截图发布，远端设备订阅显示、回传按键。
- **内置 broker** —— 进程内 rumqttd broker（TCP + WebSocket）可开机自启，零外部依赖。
- **多实例发现** —— 每个实例以 retained presence 自我公告；客户端用一条通配订阅即可发现你的全部实例。
- **Agent 状态识别** —— 解析终端标题判断 Codex / Claude Code 处于 `working` 还是 `waiting`，并随 presence 一并广播。
- **TUI 控制** —— ratatui 界面顶部有 `HTTP` / `MQTT` / `Fit` / `Quit` 按钮；`MQTT` 按钮文字反映组合状态（`off` / `brkr` / `conn` / `on`）。
- **可选 HTTP 通道** —— `/screenshot` 取图端点 + `/mqtt_ws` 调试页，按需启动。
- **跨平台** —— Linux、macOS、Windows（ConPTY）。

## 快速开始

### 1. 安装

从 [Releases 页面](https://github.com/second-state/vibetty/releases) 下载对应平台的预编译二进制，放到 `PATH` 上（推荐 `~/.cargo/bin`）。

<details>
<summary>从源码编译</summary>

```bash
git clone https://github.com/second-state/vibetty
cd vibetty
cargo build --release
# 二进制：./target/release/vibetty
```
</details>

### 2. 配置 broker（一次性）

```bash
vibetty setup
```

打开 TUI 填写 `[mqtt]` 各字段，写入 `~/.vibetty/config.toml`。最简单的是用内置 broker——无需任何外部服务：

```toml
[mqtt]
enable = true
builtin_broker = true
builtin_port = 1883      # 内置 broker TCP 端口
builtin_ws_port = 9001   # 内置 broker WebSocket 端口
```

或指向你自己的 broker / 免费 MQTT 云服务：

```toml
[mqtt]
enable = true
broker = "mqtt://user:pass@broker.example.com:1883"   # mqtts:// 走 TLS
```

> 没有 `[mqtt]` 段时，vibetty 不启用 MQTT，也不会连 broker。

### 3. 运行

```bash
vibetty -- claude        # 分享一个 `claude` 会话
vibetty -- codex         # 分享一个 `codex` 会话
```

**必须**带 `-- <命令>`。TUI 顶部的 `MQTT` 按钮显示 `conn` 即表示已连接、正在分享。

让它在后台跑、不占用当前终端：

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
tmux capture-pane -t vibetty -p | tail -20   # 确认已起来
```

提示：`vibetty skill install --claude` 会装一个 `run-vibetty` skill，里面是完整的后台会话操作流程，可直接交给 agent。

## 命令行参考

```
vibetty -- <命令>                          运行模式：在 TUI 里分享给定命令（默认）
vibetty setup                               用 TUI 配置 [mqtt]
vibetty skill install    --claude [--codex] 安装 run-vibetty skill
vibetty skill uninstall  --claude [--codex] 移除 run-vibetty skill
```

运行模式选项：

| 参数 | 说明 | 默认值 |
|---|---|---|
| `-- <命令>` | 在 PTY 里跑的程序（如 `-- claude`） | _(必填)_ |
| `--config <PATH>` | 覆盖配置文件路径 | `~/.vibetty/config.toml` |
| `-b, --bind-addr <ADDR>` | HTTP 监听地址（作为对话框预填默认值） | `0.0.0.0:3000` |
| `-a, --auto-submit` | 收到 `input_text` 时自动追加回车并执行 | `true` |
| `-q, --quality <档位>` | 屏幕输出格式:`text`(ANSI 文本流,发 `P/screen_text`)或 `high`/`medium`/`low`(JPEG 质量) | `text` |

## HTTP 端点（按需启动）

HTTP 服务**默认关闭**，在 TUI 里点 `HTTP` 按钮启动。

- `GET /screenshot` —— 当前终端画面图片（JPEG；质量由 `-q, --quality` 决定）。
- `GET /mqtt_ws` —— 浏览器 MQTT-over-WebSocket 查看页：连内置 broker 的 WS 端口、发现实例、显示画面、可发输入。没有硬件时用它测很方便。

## 当作 Herdr 插件用

vibetty 自带 Herdr 插件 manifest。一条命令装进 herdr——会拉仓库、编译 vibetty、
注册插件(`vibetty` 二进制也会落到 PATH 上,因为 herdr 通过 PATH 解析 pane 命令):

```bash
herdr plugin install second-state/vibetty
```

本地开发就 clone 后用 link:

```bash
git clone https://github.com/second-state/vibetty && cd vibetty
herdr plugin link .
```

然后配一次 MQTT(`vibetty setup`),从 herdr 命令面板跑 `share` action——或绑一个键(见下)。

### 绑定快捷键

把下面这段加到 `~/.config/herdr/config.toml`(`key = ""` 表示不绑定——自己挑一个,
比如 `prefix+v`):

```toml
[[keys.command]]
key = "prefix+v"
type = "plugin_action"
command = "vibetty.share"
description = "Share this agent pane over Vibetty"
```

然后重载 herdr 配置(`herdr server reload-config`,或重启)。在任意 agent pane 按那个
键就开出 vibetty 状态条,显示 `<agent> ▸ <pane> · [MQTT · <X.XX MB>] · <title>`
(`[MQTT ...]` 连上后变绿)。在那个 pane 里按 `q`(或 `Ctrl+C`)停止分享。

## 文档

- **[详细使用文档](USAGE.zh.md)** —— 配置、TUI、完整 MQTT 协议、ESP32 / MCU 对接指南、调试与常见问题。（[English](USAGE.md)）
- [中文更新日志](CHANGELOG.zh-CN.md) · [English changelog](../CHANGELOG.md)

## 平台支持

Linux、macOS、Windows（ConPTY）。
