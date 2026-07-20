# vibetty 使用文档

> English version: [USAGE.md](USAGE.md)

> 本文是 vibetty 的完整使用文档：安装、配置、运行、TUI 操作、HTTP 端点、**完整 MQTT 协议规格**，以及把 **ESP32 / MCU** 接入的对接指南与调试方法。
>
> 协议细节以仓库 `src/mqtt.rs` 的实际代码为唯一真相源；本文是它的可读快照。

## 目录

- [1. vibetty 是什么](#1-vibetty-是什么)
- [2. 安装](#2-安装)
- [3. 配置 MQTT（`vibetty setup`）](#3-配置-mqttvibetty-setup)
- [4. 运行](#4-运行)
- [5. 命令行与配置参考](#5-命令行与配置参考)
- [6. MQTT 传输详解（协议规格）](#6-mqtt-传输详解协议规格)
- [7. ESP32 / MCU 对接指南](#7-esp32--mcu-对接指南)
- [8. `skill` 子命令](#8-skill-子命令)
- [9. 调试与常见问题](#9-调试与常见问题)
- [10. 相关源码文件](#10-相关源码文件)

---

## 1. vibetty 是什么

vibetty 在一个 PTY 里运行一个程序（`claude`、`codex`），把这张终端画面**渲染成图片**，通过 **MQTT** 发布出去；远端设备（ESP32 / MCU / 另一台机器）订阅这张图来显示实时画面，并把按键发回来。

两端都只是连**同一个 MQTT broker** 的客户端，所以：

- 你 PC 上的端口**不用对外暴露**，不用做端口映射、不用搭转发服务器。
- broker 可以**自建**（内置 rumqttd / 外部 mosquitto / EMQX），也可以用**免费 MQTT 云服务**（EMQX Cloud 等）。数据始终在你自己手里。

每个 vibetty 实例还会发一条 **presence（上线公告）**，远端设备用通配订阅即可**发现**你当前有哪些实例在线。

```
        ┌─────────────┐   PTY    ┌──────────────────────────┐
程序 ─► │  vibetty    │ ───────► │ 渲染终端画面 → JPEG        │
(claude)│  (PC, TUI)  │          │ publish {P}/screen        │ ┐
        └─────┬───────┘          │ publish {P}  (presence)   │ │
              │ 收 {P}/pty_in、   └──────────────┬───────────┘ │
              │   {P}/control                   ▼             │ MQTT
              │                          ┌─────────────┐      │
              └──────────────────────────│   broker    │◄─────┘
                                         └──────┬──────┘
                                                │
                                     ┌──────────┴──────────┐
                                     ▼                     ▼
                              ┌──────────────┐      ┌──────────────┐
                              │   ESP32/MCU   │      │ 浏览器调试页   │
                              │ (订阅 screen,  │      │ /mqtt_ws      │
                              │  发 pty_in)    │      └──────────────┘
                              └──────────────┘
```

> 0.4.0 起，MQTT 是主要的分享通道；此外还保留一条**可选的 HTTP 通道**（`/screenshot` 取图、`/mqtt_ws` 调试页），两者共用同一个 PTY 会话。

---

## 2. 安装

**方式 A：下载预编译二进制（推荐，最快）**

从 [Releases](https://github.com/second-state/vibetty/releases) 下载对应平台的预编译二进制，放到 `PATH` 上的某个目录（推荐 `~/.cargo/bin`）。

**方式 B：从源码编译**

```bash
git clone https://github.com/second-state/vibetty
cd vibetty
cargo build --release
# 二进制：./target/release/vibetty
```

验证：

```bash
vibetty --help
vibetty --version
```

---

## 3. 配置 MQTT（`vibetty setup`）

MQTT 只在配置文件里**有 `[mqtt]` 段**时启用；否则 vibetty 完全不碰 MQTT。配置文件默认在 `~/.vibetty/config.toml`（可用 `--config <PATH>` 覆盖）。

### 交互式配置（推荐）

```bash
vibetty setup
```

打开一个 ratatui TUI，编辑 `[mqtt]` 的全部字段，回车写回配置（保留其它段）。

### 手动配置

直接编辑 `~/.vibetty/config.toml`。三种典型方案：

**方案 1：内置 broker（最省事，零外部依赖）**

```toml
[mqtt]
enable = true
builtin_broker = true
builtin_port = 1883      # 内置 broker 的 TCP 端口
builtin_ws_port = 9001   # 内置 broker 的 WebSocket 端口
```

vibetty 启动时自动在进程内起一个 rumqttd broker，自己的 client 连本地 `mqtt://127.0.0.1:1883`，ESP32 直连你 PC 的 `1883`（需要 ESP32 能访问到 PC，比如同一局域网）。内置 broker 匿名、监听 `0.0.0.0`，**仅内网使用，勿暴露公网**。

**方案 2：自建外部 broker（mosquitto / EMQX / rumqttd）**

```toml
[mqtt]
enable = true
broker = "mqtt://username:password@your-broker-host:1883"
```

broker 账号密码直接写在 URL 里（`mqtt://user:pass@host:port`）。TLS 用 `mqtts://`（默认 8883 端口、自动开 TLS）。

**方案 3：免费 MQTT 云服务**

```toml
[mqtt]
enable = true
broker = "mqtts://user:pass@broker.emqx.io:8883"
```

单用户场景下，注册一个免费 MQTT 云服务就够用了。

> 即便开了内置 broker，只要 `broker` 字段填了值，client 就连填的那个地址（不会强制改本地）。只有 `broker` 为空**且** `builtin_broker=true` 时，才默认填本地 `mqtt://127.0.0.1:{builtin_port}`。

### `[mqtt]` 字段说明

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `enable` | bool | `true` | 进程启动时是否自动起 MQTT 传输 client（关掉可保留配置但不连）。 |
| `broker` | string | _(空)_ | broker URL：`mqtt://[user:pass@]host:port` 或 `mqtts://...`。账号/密码/TLS/端口都从 URL 解析。 |
| `qos` | u8 | `1` | 预留字段。当前未生效：入站 QoS 在代码里写死（`pty_in=0`、`control=1`）。 |
| `keep_alive_secs` | u64 | `30` | MQTT keep-alive 秒数。 |
| `builtin_broker` | bool | `false` | 进程启动时是否自动起内置 rumqttd broker。 |
| `builtin_port` | u16 | `1883` | 内置 broker 的 TCP 端口。 |
| `builtin_ws_port` | u16 | `9001` | 内置 broker 的 WebSocket 端口（`/mqtt_ws` 调试页连这个）。 |

> 没有单独的 `username`/`password` 字段：账号密码写在 `broker` URL 里（`mqtt://user:pass@host`）。

---

## 4. 运行

### 4.1 基本运行

```bash
vibetty -- claude        # 分享一个 claude 会话
vibetty -- codex         # 分享一个 codex 会话
```

⚠️ **必须带 `-- <命令>`**。如果只写 `vibetty`，PTY 没有程序可跑、立刻 EOF，vibetty 会随即退出——容易误以为是端口冲突或启动失败。

启动后进入 ratatui TUI：屏幕上就是被分享的终端画面，**顶部第一行**是按钮行 `HTTP | MQTT | Fit | Quit`。

- `MQTT` 按钮文字反映组合状态：`off`（client、broker 都没跑）/ `brkr`（只起了 broker）/ `conn`（client 已连）/ `on`（broker + client 都在跑）。
- 显示 `conn` 即表示已连上 broker、正在分享屏幕。

### 4.2 后台运行（tmux）

让分享在后台持续运行、不占用当前终端：

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
sleep 6
tmux ls                                            # 应能看到名为 vibetty 的会话
tmux capture-pane -t vibetty -p | tail -20         # 确认 claude 已起来、MQTT 按钮显示 conn
```

- `-s vibetty`：tmux 会话名，可自定义；同时分享多个会话就各起一个不同名字。
- `-c "<目录>"`：终端会话的工作目录。**用 `-c`，不要用 `cd ... && tmux`**（已有 tmux server 时后者不会按预期切目录）。
- `tmux attach -t vibetty` 进入查看/操作；`Ctrl-b d` 脱离（会话继续后台跑）；`tmux kill-session -t vibetty` 结束。

### 4.3 TUI 操作（顶部按钮 + MqttPanel）

顶部按钮行（屏幕第 1 行）`HTTP | MQTT | Fit | Quit`，鼠标可点击、悬停高亮：

- **HTTP**：按需启停 HTTP 服务（默认关闭）。点开时让你确认监听地址（预填 `--bind-addr`）。
- **MQTT**：点开 **MqttPanel** 弹窗，分上下两块：
  - **Broker 块**：`TCP:` / `WS :` 端口可编辑（回车存回 config）；`Start broker` 起内置 rumqttd（⚠️ **只能起不能停**——rumqttd 无 shutdown API，起来后变只读 `● broker running :{port}`）。
  - **Client 块**：`URL:` broker 地址可编辑（回车存回 `[mqtt] broker`）；`Start client` / `Stop client` 起/停传输 client。
  - `Tab` / `↑↓` 在各项间循环；回车行为随聚焦项变（端口=存盘、BrokerStart=起 broker、URL=存 URL、ClientToggle=切 client）。
- **Fit**：一键把终端尺寸重置为当前窗口尺寸（扣掉按钮行 + 上边框后的可用区）。
- **Quit**：直接退出 TUI。

> 停 client 期间的消息不会缓存、重启不补发；真正的下线清理靠 **LWT**（空 retained）：连接一断，broker 自动清掉 presence。

### 4.4 HTTP 端点（按需启动）

HTTP 服务**默认关闭**，点 `HTTP` 按钮启动（监听地址预填 `-b/--bind-addr`，默认 `0.0.0.0:3000`）：

- `GET /screenshot` —— 返回当前终端画面图片（JPEG）。带 `Cache-Control: no-cache`，可轮询取最新画面。
- `GET /mqtt_ws` —— 浏览器 MQTT-over-WebSocket 调试页：用 mqtt.js 连**内置 broker 的 WS 端口**（默认 `:9001`），自动发现实例、显示屏幕图片、可发输入。**没有 ESP32 硬件时用它验证最方便。**

> ⚠️ 这两个端点只在你显式启动 HTTP 服务后才存在；不启动则没有任何对外 HTTP 服务。

---

## 5. 命令行与配置参考

### 子命令

```
vibetty -- <命令>                          运行模式：在 TUI 里分享给定命令（默认）
vibetty setup                               用 TUI 配置 [mqtt]
vibetty skill install    --claude [--codex] 安装 run-vibetty skill
vibetty skill uninstall  --claude [--codex] 移除 run-vibetty skill
```

### 运行模式选项

| 参数 | 说明 | 默认值 |
|---|---|---|
| `-- <命令>` | 在 PTY 里跑的程序（如 `-- claude`、`-- codex`）。**必填**。 | _(必填)_ |
| `--config <PATH>` | 覆盖配置文件路径。 | `~/.vibetty/config.toml` |
| `-b, --bind-addr <ADDR>` | HTTP 监听地址（启动 HTTP 时作为对话框预填默认值）。 | `0.0.0.0:3000` |
| `-a, --auto-submit` | 收到 `input_text` 时自动追加回车并执行（同时把 scrollback 设为 3，露出一点历史）；关掉则只输入文本、scrollback=0（最新）。 | `true` |
| `-q, --quality <档位>` | 屏幕输出格式:`text`(默认;在 `P/screen_text` 发纯 ANSI 文本流,不出图——见 §6.5b),或 `high`(JPEG q85 彩色)、`medium`(JPEG q70 彩色)、`low`(JPEG q50 黑白)。 | `text` |

### 截图渲染参数（固定）

终端画面按以下参数渲染成图片（`SCREEN_*` 常量，见 `src/ws.rs`）：

- 字符单元：**8 × 18 像素**（宽 × 高）。
- 四周留白：**各 16 像素**。
- 整图尺寸 = `cols × 8 + 32`（宽）× `rows × 18 + 32`（高）。
- 编码为 JPEG；质量档位由 `-q, --quality` 决定（见上方「运行模式选项」）：`high` = q85 彩色、`medium` = q70 彩色、`low` = q50 黑白。默认是 `-q text`,屏幕改发 ANSI 文本流、不出 JPEG。

---

## 6. MQTT 传输详解（协议规格）

> 本节是远端客户端（ESP32 / MCU / 任意 MQTT 客户端）必须遵守的契约。有任何不确定，以 `src/mqtt.rs` 实际代码为准。

### 6.1 Broker 连接

- vibetty 是 MQTT 客户端，连 `[mqtt] broker` 指定的 broker。
- **远端设备必须连同一个 broker**（同样的 host/port/账号）。
- 认证：用 broker 的 username/password（写在 vibetty 的 `broker` URL 里）。远端用同一个账号（或 broker 上另一个有权限的账号）。
- 端口：`1883` 明文 / `8883` TLS（`mqtts://` 自动开 TLS）。
- 最大报文 **1MB**（client 与内置 broker 都是这个上限；外部 broker 也要 ≥ 1MB，否则 `screen` 图会被截断）。

### 6.2 Topic 命名（关键 ⚠️）

每个 vibetty 实例的所有 topic 都挂在一个**自动构造的前缀**下：

```
{user}/{device}/{pid}/vibetty
```

| 段 | 来源 | 性质 |
|----|------|------|
| `user` | broker URL 里的 `username`；没填账号则回退 **`root`** | 稳定（多租户隔离） |
| `device` | `SHA256(machine-uid)` 前 16 hex（PC 机器指纹） | 稳定（跨重启不变）+ 跨机器唯一 |
| `pid` | vibetty 进程 pid | **每次重启变** |

**为什么远端必须做 discovery**：`device` 是 PC 的机器指纹（远端算不出）、`pid` 每次变（远端无法预测）。所以远端**不能预知实例的 topic**，必须先通过 presence 发现（见 6.5）。

### 6.3 Topic 清单

设 `P = {user}/{device}/{pid}/vibetty`（实例前缀）：

| 方向 | topic | payload | QoS / retained | 说明 |
|------|-------|---------|----------------|------|
| 设备 → vibetty | `P/pty_in` | **raw bytes** | QoS 0 / 否 | 原始按键字节（单键 / 转义序列） |
| 设备 → vibetty | `P/control` | **JSON** | QoS 1 / 否 | 控制消息（输入文本 / 同步 / 滚动） |
| vibetty → 设备 | `P/screen` | **raw bytes**（JPEG + 末尾 4 字节 offset） | QoS 0 / **是** | 整张 JPEG 屏幕帧。**仅 JPEG 模式**（`-q high/medium/low`）。 |
| vibetty → 设备 | `P/screen_text` | **raw bytes**（`[1 字节 tag] + ANSI 文本`，见 6.5b） | QoS 0 / 全屏帧 **是**、增量帧 **否** | text 模式屏幕流（全屏基线 + 实时 pty 增量）。**仅 text 模式**（`-q text`）。 |
| vibetty → 设备 | `P`（前缀本身） | **JSON** | QoS 1 / **是** | presence 公告（服务发现） |

> **两种输出模式**（`-q`,vibetty 启动时决定,整条会话固定):实例**要么**发 `P/screen`(JPEG,`-q high/medium/low`),**要么**发 `P/screen_text`(text,`-q text`),不会两个都发。远端按 presence 的 `format` 字段决定订哪个(见 6.7)。**没有独立的 `P/pty_out` topic**——text 模式的实时 pty 字节作为增量帧(tag `0x01`)合在 `P/screen_text` 里。
>
> `screen` 和 `screen_text` 的全屏帧都是 **retained**:远端一订阅就立即收到最近一帧。`screen_text` 的**增量帧不 retained**(这样 retained 的永远是完整基线)。

### 6.4 `control` 的 JSON 格式

复用 vibetty `ClientMessage` 的 serde 形式（`#[serde(tag="type", content="data")]`，靠 `type` 区分）。远端只需发这 4 种：

| type | data | 含义 |
|------|------|------|
| `input_text` | 字符串 | 输入一段文本（如命令）。若服务端开了 `--auto-submit`，会自动追加回车执行。 |
| `sync` | `{"width":W,"height":H,"pixels":bool,"close":bool}` | 声明远端显示尺寸 + 控制自主推送。`pixels`(默认 `true`):`width`/`height` 是**像素** → 服务端换算 cols/rows;`false`:已是字符列/行,直接用。`close`(默认 `false`):`true` = **暂停**服务端自主推屏(远端不看时省流量);`false` = 恢复。服务端 resize PTY 并回送一帧整屏(见 6.6)。 |
| `scroll_up` | `{"rows":N}` | 向上滚动；`rows`=0 / 缺省 = 滚一整页（= 终端可见行数,留两行重叠）。 |
| `scroll_down` | `{"rows":N}` | 向下滚动；同上。 |

`sync` 的 `pixels` / `close` 都是**可选、向后兼容**的(缺省 → `pixels:true`、`close:false`)。示例：

```json
{"type":"input_text","data":"ls -la\n"}
{"type":"sync","data":{"width":320,"height":240}}
{"type":"sync","data":{"width":80,"height":24,"pixels":false}}
{"type":"sync","data":{"width":80,"height":24,"pixels":false,"close":true}}
{"type":"scroll_up","data":{"rows":0}}
```

> `pty_in`（raw 单键）和 `control` 的 `input_text`（文本串）的区别：**单键 / 方向键 / 控制字符**走 `pty_in` 的 raw 字节；**整段文本 / 命令行**走 `control` 的 `input_text`。

### 6.5 `screen` 的 payload（JPEG 模式）

整张 JPEG 图片字节，**无分块、无信令字段**。但**末尾追加了 4 字节**，需要注意：

1. 图片开头是 JPEG magic bytes `FF D8 FF`，按 JPEG 解码即可。
2. 解码出图片后，**读末尾 4 字节**作为 `scrollback offset`（u32 **大端** = 网络序）：
   - `0` = 这张图截自**底部 / 最新**位置；
   - `> 0` = 截自向上滚动了 N 行的位置。
   - 这 4 字节在图片的 `EOI`（JPEG）之后，解码器会忽略，所以不影响解码。

### 6.5b `screen_text` 的 payload（text 模式）

text 模式(`-q text`)把屏幕作为 **ANSI 终端流**发到 `P/screen_text`——「全屏基线 + 实时增量」设计。每个 payload 开头是 **1 字节 tag**:

```
payload = [ tag: 1 字节 ] [ 内容 ]
```

| tag | 含义 | 内容 | retained | 何时发 |
|-----|------|------|----------|--------|
| `0x00` | **全屏基线** | vt300 `contents_formatted()` 的输出:可重放的 ANSI 流(光标归位 + SGR 颜色 + 文本),喂给空终端解析器即可还原整屏(含颜色) | **是** | 启动首帧,以及每次 `sync` / `scroll_*` 的响应 |
| `0x01` | **pty 增量**(实时) | 原始 PTY 输出字节(ANSI 转义 + 文本) | **否** | PTY 每次有输出就发(实时) |

- **为什么增量不 retained**:这样 broker 上 retained 的永远是完整的 `0x00` 基线。重连时远端拿到的是可用的全屏帧,而不是一个会在空白 buffer 上产生乱码的陈旧增量。
- **远端推荐实现**:维护一个终端模拟器 buffer;收到 `0x00` 就 reset 后重放整屏;收到 `0x01` 就把字节增量喂进去(就像真终端接收 shell 输出)。连上后 retained 的 `0x00` 提供基线;不放心就发一条 `sync` 强制再发一个 `0x00`。
- 内容**含 ANSI 转义**(`\x1b[...`)——不是纯文本。要么用终端解析器渲染,要么自己剥离转义。
- `close=true` 会停掉 `0x01` 增量(自主推送暂停);`0x00` 全屏帧在 `sync` / `scroll_*` 响应里照发。

### 6.6 `sync` 的尺寸换算 + `close` 开关

**尺寸单位**取决于 `pixels` 字段:

- `pixels: true`(默认):`width`/`height` 是**像素**。服务端按截图渲染参数换算(见第 5 节):

  ```
  cols = (width  - 32) / 8      # 32 = 左右各 16px 留白；8 = 字符宽
  rows = (height - 32) / 18     # 18 = 字符高
  ```

- `pixels: false`:`width`/`height` 已是**字符列/行**,服务端直接用。

最低 `cols = 8`、`rows = 2`（防 vt100 0 行 panic）。远端只要如实上报自己的显示尺寸,服务端自动 resize。

**`close` 开关**(省流量):控制服务端**自主**推屏(PTY 输出触发的那部分):

| `close` | JPEG 模式 | text 模式 |
|---------|-----------|-----------|
| `false`(默认) | PTY 输出停顿满 100ms → 发一帧 `P/screen` | 每次 PTY 输出 → 发一条 `P/screen_text` 增量(`0x01`) |
| `true` | 停止发 `P/screen`(在途帧也丢) | 停止发增量(`0x01`) |

不受 `close` 影响(客户端主动请求,照常回送):`sync` 的屏幕响应、`scroll_*` 响应、presence 心跳。典型用法:远端息屏 / 用户没在看时发 `close=true` 静音推流;`close=false` 恢复。

### 6.7 Discovery / presence 机制

vibetty 上线时，在 `P`（前缀本身）发一条 **retained** presence：

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

| 字段 | 含义 |
|------|------|
| `prefix` | 完整实例前缀（远端据此订阅输出通道） |
| `client_id` | vibetty 的 MQTT client id（调试用） |
| `ts` | 当前 epoch 秒（远端据此判活） |
| `title` | 终端窗口标题（程序通过 OSC 设置），用于 agent 状态识别 |
| `state` | agent 工作状态：`"working"` 或 `"waiting"`（小写）。Codex / Claude Code 在等用户操作时为 `waiting`。 |
| `format` | **输出模式**:`"high"` / `"medium"` / `"low"`(JPEG → 订阅 `P/screen`)或 `"text"`(text → 订阅 `P/screen_text`)。据此决定订阅哪个屏 topic。 |

- **每 15s 重发一次**（心跳，刷新 `ts`）。
- **异常掉线**：broker 触发 LWT，向 `P` 发一条**空 payload**（= 删除 retained），远端立即知道实例下线。
- **agent 状态翻转**（working↔waiting）会**立刻重发** presence，远端可据此决定要不要把画面推给用户。

**远端的发现订阅**：

- 若远端知道 `user`（= 它自己连 broker 的 username，且 vibetty 也用了同一个）：subscribe `{user}/+/+/vibetty`（`+` 通配 device 和 pid 两段）。
- 若不知道 user（vibetty 没在 broker URL 里填账号，user 段回退 `root`）：用更宽的 `+/+/+/vibetty`（通配 user/device/pid 三段）。
- retained 保证远端一连上就**立即收到所有现存实例**的 presence。

---

## 7. ESP32 / MCU 对接指南

本节给在 ESP32 仓库工作的同学。目标：让 ESP32 通过 MQTT 连上 PC 上的 vibetty，实现「发现实例 + 显示画面 + 收发输入」。

### 7.1 要实现的功能清单

1. ✅ 连接 broker（host/port/认证与 vibetty 配置一致）。
2. ✅ **Discovery**：subscribe presence 通配 topic，解析 payload，维护「在线实例列表」。
3. ✅ 选定目标实例后,**按它的 `format`** 订阅屏 topic:`"text"` → `{P}/screen_text`;否则(JPEG)→ `{P}/screen`。(无屏可只发输入、不订阅省带宽。)
4. ✅ **JPEG 模式**:收 `screen` → 据 magic bytes 判 JPEG → 解码显示 → 读末 4 字节 scrollback offset。**text 模式**:收 `screen_text` → 读首字节(`0x00` = 全屏基线 → reset 终端 buffer 后重放;`0x01` = pty 增量 → 增量喂进 buffer)。
5. ✅ 发单键 → publish `{P}/pty_in`（raw bytes）。
6. ✅ 发文本命令 → publish `{P}/control`（JSON `input_text`）。
7. ✅ 上报显示尺寸(+ 省流量开关)→ publish `{P}/control`(JSON `sync`,带 `width`/`height`/`pixels`/`close`)。
8. ✅ **存活判断**：presence 的 `ts`（超过 ~30s 未更新当离线）+ LWT 空 payload（实例下线，立即移除）。
9. ✅ **切换目标**：unsubscribe 旧实例的屏 topic(`screen` 或 `screen_text`),按新实例的 `format` subscribe。
10. ✅（可选）**agent 状态**：据 presence 的 `state` 决定是否需要把画面推给用户。
11. ✅(可选,**省流量**)**`close` 开关**:息屏 / 用户没在看时,发带 `close=true` 的 `sync` 暂停服务端自主推送;`close=false` 恢复。

### 7.2 代码骨架（`esp-idf-svc`，结构参考）

用 `EspAsyncMqttClient`。API 细节以 `esp-idf-svc` 最新文档为准。

**连接 + 发现订阅**：

```rust
use embedded_svc::mqtt::client::{AsyncClient, QoS};          // subscribe/unsubscribe/publish
use esp_idf_svc::mqtt::client::{EspAsyncMqttClient, MqttClientConfiguration};

let mut client = EspAsyncMqttClient::new(
    "mqtt://broker.example.com:1883",      // 或 mqtts://...:8883
    &mut MqttClientConfiguration {
        client_id: Some("vibetty-esp32-001"),   // broker 内必须唯一
        username: Some("root"),                  // 与 vibetty 同一 broker 账号
        password: Some("secret"),
        buffer_size: 32 * 1024,                  // ⚠️ 必须配大，见 7.3
        out_buffer_size: 8 * 1024,
        ..Default::default()
    },
)?;

// Discovery：订阅 presence（retained → 立即收到现存实例）
client.subscribe("+/+/+/vibetty", QoS::AtLeastOnce).await?;
```

**收消息主循环**：

```rust
let mut current_prefix: Option<String> = None;
let mut current_format: Option<String> = None;   // 实例的 `format`("text" / "high" / ...)

loop {
    let msg = client.next().await?;
    let topic = msg.topic();
    let payload = msg.payload();
    let segs: Vec<&str> = topic.split('/').collect();

    match segs.as_slice() {
        // presence: [user, device, pid, "vibetty"]（4 段）
        [_, _, _, "vibetty"] => {
            if payload.is_empty() {
                // LWT：实例下线 → 清空当前目标
                current_prefix = None;
            } else {
                // {"prefix","client_id","ts","title","state","format"}
                let p: Presence = serde_json::from_slice(payload)?;
                if current_prefix.as_deref() != Some(&p.prefix) {
                    // unsubscribe 旧实例的屏 topic(看之前是哪个)
                    if let Some((old, fmt)) = current_prefix.take().zip(current_format.take()) {
                        let topic = if fmt == "text" { "screen_text" } else { "screen" };
                        client.unsubscribe(&format!("{old}/{topic}")).await?;
                    }
                    current_prefix = Some(p.prefix.clone());
                    current_format = Some(p.format.clone());
                    // 按实例的 `format` 订阅对应的屏 topic
                    let topic = if p.format == "text" { "screen_text" } else { "screen" };
                    client.subscribe(&format!("{}/{topic}", p.prefix), QoS::AtLeastOnce).await?;
                    // 上报自己的显示尺寸，让服务端 resize
                    client.publish(
                        &format!("{}/control", p.prefix),
                        QoS::AtLeastOnce, false,
                        br#"{"type":"sync","data":{"width":320,"height":240}}"#,
                    ).await?;
                }
            }
        }
        // JPEG 屏幕图: [..., "vibetty", "screen"]
        [.., "vibetty", "screen"] => {
            // 1) 据 magic bytes 判 JPEG → 解码
            // 2) 读末 4 字节 = scrollback offset（0 = 最新）
            // 3) 显示
        }
        // text 屏幕流: [..., "vibetty", "screen_text"]
        [.., "vibetty", "screen_text"] => {
            // payload[0] 是 tag:
            //   0x00 = 全屏基线 → reset 终端 buffer,重放 payload[1..]
            //   0x01 = pty 增量 → 把 payload[1..] 增量喂进 buffer
        }
        _ => {}
    }

    // 存活兜底：另起定时器，current_prefix 对应实例的 ts 超过 30s 未更新 → 清空目标
}
```

**发输入**：

```rust
// 单键 / raw 字节 → pty_in
client.publish(&format!("{prefix}/pty_in"), QoS::AtLeastOnce, false, &[b'a']).await?;

// 文本命令 → control（JSON）
client.publish(
    &format!("{prefix}/control"),
    QoS::AtLeastOnce, false,
    br#"{"type":"input_text","data":"ls -la\n"}"#,
).await?;
```

### 7.3 关键细节 / 坑

1. **buffer 必须配大**：`screen` 是整张 JPEG（可达几十 KB），ESP-IDF mqtt 默认 `buffer_size` 不够会截断。`MqttClientConfiguration` 的 `buffer_size`（收）至少给 32KB，按实际截图大小调。
2. **二进制 OK**：`pty_in` / `screen` 都是 raw bytes，esp-mqtt 支持 binary payload。`control` 是 JSON（UTF-8）。
3. **retained 的正确处理**：首次 subscribe presence / screen 时，broker 会把现存的 retained 一次性推过来，所以远端一连上就有完整在线列表 + 最近一张画面。
4. **LWT 空 payload = 删除**：收到一条 payload **为空**的 presence 消息，就是实例下线信号，立即从列表移除。
5. **ts 判活兜底**：LWT 只在异常断连触发；正常退出靠 `ts`。远端维护「最后见到的 ts」，**`now - ts > 30s`** 当离线（注意 ESP32 时钟要准，或用 broker 时间）。
6. **pid 跨重启变**：不要把 prefix 持久化缓存，每次启动重新 discovery。
7. **unsubscribe 是异步的**：返回时只是包已发，broker ACK 前可能还收几条该 topic 的消息，要能容忍（忽略已退订 topic）。
8. **TLS**：8883 用 `mqtts://`；ESP32 直连 TCP MQTT，不走 WebSocket。
9. **`user` 段的确定**：远端连 broker 的 username **就是** vibetty 的 `user` 段（前提：vibetty 在 broker URL 里填了账号且账号一致）。若 vibetty 没填账号（user 段 = `root`），远端用宽通配 `+/+/+/vibetty`。
10. **ASR 在 ESP32 本地做**：服务端不做语音转写。ESP32 识别完把**文本**经 `control` 的 `input_text` 发回即可，省音频流量。
11. **text 模式(`format: "text"`)**:订阅 `P/screen_text`(不是 `P/screen`),按首字节 tag 分派(`0x00` 全屏 / `0x01` 增量)——见 §6.5b。增量高频且**不 retained**,retained 的永远是 `0x00` 基线;终端 buffer 失同步就发一条 `sync` 强制重发 `0x00`。内容含 ANSI 转义(要终端解析器,不能直接 `print`)。不看时用 `sync.close=true` 静音实时增量。

### 7.4 不依赖 ESP32 也能验证（本地）

先把 vibetty 侧跑通，再用浏览器调试页 / Python 模拟远端验证协议：

**① 起 broker + vibetty**

最省事的是开内置 broker（`[mqtt] builtin_broker = true`），后台跑起 vibetty：

```bash
tmux new-session -d -s vibetty -c "$HOME/workspace" 'vibetty -- claude'
```

（用外部 broker也行：`mosquitto -c /tmp/mosq.conf`，最小配置 `listener 1883 127.0.0.1` + `allow_anonymous true`。）

**② 浏览器调试页（推荐，零代码）**

启动 HTTP 服务（TUI 点 `HTTP` 按钮），浏览器开 `http://localhost:3000/mqtt_ws`：它自动连内置 broker 的 WS 端口、发现实例、显示画面、可发输入。

**③ Python `paho-mqtt` 模拟远端**

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

# 发现（retained → 立即收到现存实例）
c.subscribe("+/+/+/vibetty")
c.loop_start(); time.sleep(2)

# 把 <prefix> 换成上一步 on_msg 打印的 prefix 值
prefix = "root/1a2b3c4d5e6f7a8b/12345/vibetty"
c.publish(f"{prefix}/pty_in", b"l")                                  # 单键
c.publish(f"{prefix}/control", json.dumps({"type":"input_text","data":"ls\n"}))
time.sleep(2)
```

vibetty 的终端应出现 `l` 和 `ls` 命令的输出。

**④ 测 LWT**：`tmux kill-session -t vibetty`，看 Python / mosquitto_sub 是否收到一条**空 payload**（presence 被删）。

> 本机 sandbox 下 `mosquitto_pub/sub` CLI 有时报 "Bad file descriptor"，用 Python `paho-mqtt` 更顺。

---

## 8. `skill` 子命令

把内置的 `run-vibetty` SKILL.md 装进 / 移出 agent 的**用户级** skills 目录——装好 vibetty 后一条命令搞定，不用手动复制 skill 文件夹。skill 内容是教 agent「后台 tmux 起 vibetty 会话、经 MQTT 把终端画面分享给 ESP32」。

```bash
vibetty skill install --claude          # → ~/.claude/skills/run-vibetty/
vibetty skill install --codex           # → ~/.agents/skills/run-vibetty/（Codex USER scope）
vibetty skill install --claude --codex  # 两个都装
vibetty skill uninstall --claude        # 移除（目录随后为空才删目录）
```

- `--claude` / `--codex` 是 bool flag，可同时给；都不给 → 报错退出。
- **版本感知**：install 前比 `CARGO_PKG_VERSION` 与目标目录下伴生文件 `.vibetty-version`。同版本跳过；版本不同 / 无记录 → 覆盖升级。版本号唯一真相源是 `Cargo.toml`，发版自动跟随。
- **uninstall 安全**：删 `SKILL.md` + `.vibetty-version`，只在目录随后变空时才删目录（**绝不**用 `remove_dir_all`，避免误删 `~/.claude/skills/` 或 `~/.agents/skills/`）。
- Codex 路径是 `~/.agents/skills/`（不是 `~/.codex/`），见 developers.openai.com/codex/skills 的 USER scope。

---

## 9. 调试与常见问题

**日志在哪？**
vibetty 用 flexi_logger 把日志写到 **CWD**（启动时所在的目录，或 tmux `-c` 指定的目录），文件名 `vibetty*.log`，追加写、达 10MB 轮转。**不写 stdout**。排查问题去那里看。

**没配 `[mqtt]` 会有问题吗？**
不会。`~/.vibetty/config.toml` 不写 `[mqtt]` 段，启动应**零 MQTT 日志**，TUI / PTY 行为不变。

**启动后秒退 / `tmux ls` 报 no server running？**
大概率是**没带 `-- <命令>`**。PTY 没程序可跑、立刻 EOF，vibetty 随即退出，tmux 会话几秒内消失。这容易被误判成端口冲突或启动失败。

**HTTP 怎么访问不了？**
HTTP 服务**默认关闭**，要在 TUI 点 `HTTP` 按钮启动。不启动就没有 `/screenshot`、`/mqtt_ws`。

**ESP32 收不到画面？**
依次排查：
1. ESP32 与 vibetty 连的是**同一个 broker**（host/port/账号一致）？
2. ESP32 的 `buffer_size` 够大（≥ 32KB）？默认值会截断 `screen`。
3. discovery 通配对了吗？vibetty 没填 broker 账号时 user 段是 `root`，用 `+/+/+/vibetty`。
4. vibetty 的 PTY 有没有输出触发广播？headless 下要 PTY 有输出才会发 `screen`。
5. 内置 broker 监听 `0.0.0.0`，确认 ESP32 网络能访问到 PC 的 1883 端口（防火墙等）。

**内置 broker 能停吗？**
不能。rumqttd 没有 shutdown API，`Start broker` 只能起、不能停。停整个 vibetty 进程才会带走它。

**改了 `qos` 字段没反应？**
正常。`[mqtt] qos` 是预留字段，当前未生效；入站 QoS 在代码里写死（`pty_in=0`、`control=1`）。

**headless / 无 TTY 下能跑吗？**
能跑，80×24 默认尺寸已兜底（不会再 panic）。但要收到 `screen` 仍需 PTY 有输出触发广播。后台 tmux 是最稳的跑法。

---

## 10. 相关源码文件

| 文件 | 作用 |
|---|---|
| `src/mqtt.rs` | MQTT 桥接：topic 构造（`instance_prefix`）、presence（`presence_payload`）、LWT、心跳、`parse_control`、`INBOUND_TOPICS`、`screen` 渲染发布。**协议以此文件为准。** |
| `src/config.rs` | `Cli` / `RunArgs` / `MqttConfig`（全部 `[mqtt]` 字段）、`MqttConfig::for_client()`（URL 解析，唯一一处）、`mqtt_config()`（读配置）。 |
| `src/protocol.rs` | `ClientMessage`（`control` JSON 的 serde 来源）、`ServerMessage`、`ImageFormat`。 |
| `src/terminal/agent.rs` | `AgentState`（Working/Waiting）、按终端标题识别 Codex / Claude Code 状态。 |
| `src/ws.rs` | `run_command`（主循环：PTY、TUI、按钮、boot 自动起 MQTT、agent 状态广播）、`SCREEN_*` 渲染常量、`render_screen_to_image`、`/screenshot` + `/mqtt_ws` 路由。 |
| `src/broker.rs` | `spawn_builtin()`：独立线程跑 rumqttd（TCP + WS，匿名，1MB payload）。无 shutdown。 |
| `src/setup.rs` | `vibetty setup` 的 ratatui TUI，编辑 `[mqtt]` 全部字段写回 config。 |
| `src/skill.rs` | `vibetty skill install/uninstall` 实现，内嵌 `resources/skills/run-vibetty/SKILL.md`。 |
