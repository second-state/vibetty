# 变更日志 (CHANGELOG)

## [0.4.0] - 2026-07-29

第一个 MQTT 正式版。

### 新增

- **MQTT 传输**——通过 MQTT 分享终端会话,无需暴露端口。
- **文本输出模式(`-q text`)**——屏幕以 ANSI 终端流发送;JPEG 模式(`-q high/medium/low`)也可用。
- **内置 broker**——进程内 rumqttd,零外部依赖。
- **多实例发现**——通过 retained presence 自动发现。
- **agent 状态检测**——Codex / Claude Code 的 working/waiting 状态随 presence 广播。
- **Web 调试页**(`/mqtt_ws`)——MQTT-over-WebSocket 客户端,含移动端布局和键盘输入。
- **`Sync.pixels` / `Sync.close`** 字段——按像素或字符格上报尺寸;暂停/恢复自主推屏。
- **presence `format`**——告诉客户端订阅哪个屏 topic。
- **`vibetty skill`** 子命令——安装内置 SKILL.md 到 Claude Code / Codex。

### 变更

- **`-q` 默认改为 `text`**(原 `high`)。
- 屏 topic **不 retained**(只有 presence)——防止重启后残留。
- **biased select!** 优先处理入站控制消息。
- **resize burst 吸收**——重绘 burst 缓冲 500ms 后发一帧全屏。
- screen **去抖 100ms**;滚动翻页留 **2 行**重叠。
- **`JpegQuality` 改名 `OutputFormat`**。

### 修复

- Backspace 发 DEL(`0x7f`)而非 Ctrl+H。
- 导航键修饰键(Shift/Ctrl/Alt + 方向键/Home/End/PgUp/PgDn/Delete)正确编码。

让 MQTT 控制消息在狂输出时仍保持响应。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 修复

- **狂输出时控制消息不再被饿死**。MQTT 桥和主事件循环的两个 `select!` 都改成 `biased`,把入站控制(`sync`/`pty_in`/`close`)排在 PTY 输出前面。此前慢 broker 上出站 publish 太重会挤掉入站 poll,连 `close=true`(本用来停 flood 的救命消息)都进不来。
- **`close=false` 的 sync 恢复立即回送屏幕**。之前 resize-settle 改动让所有 sync 都等 500ms;现在 settle 只在 sync 真的 resize 了 PTY 时才触发,非关闭的 sync 立刻响应。

### 变更

- **text 模式每条输出更省**。redraw 闭包改借 `&Screen`(不再是 `&Arc<Screen>`),PtyOutput 处理不再前置 clone 整屏(text 模式广播的是原始字节)。整屏 clone 现在只在 JPEG 去抖路径上发生。

## [0.4.0-rc.8] - 2026-07-26

text 模式 QoS 调整 + resize burst 处理。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 变更

- **`screen_text` QoS 拆分**:全屏基线帧(tag `0x00`)改为 QoS 1(低频、值得可靠送达);实时 pty 增量(tag `0x01`)仍是 QoS 0(高频、丢一帧无所谓)。JPEG `screen` 和 `pty_in` 仍 QoS 0;presence 是 QoS 1。
- **resize 不再灌增量**。resize PTY(sync / 窗口 resize / Fit)会触发 TUI 全量重绘 burst,vibetty 现在把这 500ms 内的 burst 吸收掉(每来一段重设 500ms 计时器),等输出静默满 500ms 才发一帧全屏,而不是把每个中间帧当 pty_out 增量灌过去。正常输出不受影响。

## [0.4.0-rc.7] - 2026-07-26

默认输出模式 + retained 消息卫生。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 变更

- **`-q text` 改为默认**(原 `high`)。不传 `-q` 时,vibetty 现在把屏幕作为 ANSI 文本流发到 `P/screen_text`,不再出 JPEG 图。
- **屏 topic 不再 retained**。`{p}/screen` 和 `{p}/screen_text`(全屏 `0x00` + 增量 `0x01`)都 `retain=false`,只有 presence 仍 retained。topic 前缀带 pid、每次重启就变,retained 的屏幕帧以前会堆在没人清的老 `{old-pid}/...` topic 上。远端现在靠连上时发 `sync` 拿首帧。
- **退出不发干净 MQTT DISCONNECT**。vibetty 不再调 `client.disconnect()`,让 broker 把断开的 socket 当异常掉线 → 必发 LWT 清掉 retained presence(干净 DISCONNECT 会抑制 LWT,presence 会残留在老 pid 的 topic 上)。

## [0.4.0-rc.6] - 2026-07-20

新增 text 输出模式 + MQTT 协议扩展(原始 PTY 流、Sync 字段、format 发现)。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 新增

- **text 输出模式(`-q text`)**:除 JPEG 质量档位外,屏幕现在可作为 ANSI 终端流发到新的 `{p}/screen_text` topic,不再出 JPEG 图。每个 payload 首字节是 1 字节 tag:`0x00` = 全屏基线(`vt80` `contents_formatted`,可重放的带颜色 ANSI 流)、`0x01` = 实时原始 PTY 增量。全屏帧 retained、增量帧不 retained(重连总能拿到完整基线)。该模式下 `/screenshot` 返回 `text/plain`。
- **text 模式实时 PTY 流**:PTY 输出立即作为 `0x01` 增量发到 `{p}/screen_text`(独立的 `{p}/pty_out` topic 取消——增量并进了 `screen_text`)。JPEG 模式不变(去抖后的 `{p}/screen` 帧)。
- **`Sync.pixels` 字段**:`false` 时客户端直接发字符列/行,服务端跳过像素→字符格换算。默认 `true`(像素),向后兼容。
- **`Sync.close` 字段**:服务端自主推屏的暂停开关。`close=true` 停止 PTY 输出触发的推送(并丢弃在途去抖帧);`close=false` 恢复。默认 `false`。客户端主动请求(sync 响应、scroll)不受影响。让省电客户端不看时能静音推流。
- **presence 加 `format` 字段**:实例的 `-q` 设置(`high`/`medium`/`low`/`text`)写进 presence JSON,客户端据此决定订阅 `{p}/screen`(JPEG)还是 `{p}/screen_text`(text)。

### 变更

- **`JpegQuality` → `OutputFormat`**:枚举改名(现已覆盖非 JPEG 的 text 模式),新增 `Text` / `is_text()` / `as_str()`。已有档位的 wire 格式不变(`high`/`medium`/`low`);新增 `text`。
- **渲染决定移到 MQTT 桥**:`ws` 只广播 `Screen`;MQTT 任务按 `image_format` 渲染成 JPEG 或 ANSI 文本。`ScreenText` 不再是独立协议变体。
- **text 模式流量纳入统计**:MQTT 出站字节计数(`total_screen_bytes`)现在也累加 text 模式的全屏帧和增量帧(原先只算 JPEG);两者日志都带累计 MB。

## [0.4.0-rc.5] - 2026-07-15

全屏编辑器(helix / vim / zerostack / …)的键盘输入修复。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 修复

- **Backspace 被当成 Ctrl+H**:Backspace 键之前编码成 `0x08`(BS = Ctrl+H)而非 `0x7f`(DEL),导致按 Backspace 删字符的编辑器收到的是 Ctrl+H。现在发 DEL。
- **导航键的修饰键被丢弃**:方向键/Home/End/PgUp/PgDn/Delete/Insert 配合 Shift/Ctrl/Alt 时,修饰键被忽略、只发裸序列,导致编辑器快捷键(Shift+方向选中、Ctrl+←/→ 按词跳、Ctrl+Delete 等)失效。现在按 xterm「Modified Keys」规范编码修饰键(如 Ctrl+→ → `\x1b[1;5C`、Shift+↑ → `\x1b[1;2A`、Ctrl+Delete → `\x1b[3;5~`)。裸键(无修饰)行为不变。

## [0.4.0-rc.4] - 2026-07-15

Codex 状态检测 + 更简洁的 screen 防抖 + 滚动上下文。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 新增

- **更准的 Codex 状态检测**:Codex 的 working/waiting 状态现在从终端 title 里的盲文 spinner(⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏)读取——有 spinner 即 *working*——比之前按 title 前缀匹配更可靠地翻转状态。

### 变更

- **更简洁的 screen 防抖**:每次 PTY 输出都启动(或刷新)一个 100ms 计时器,输出静默满 100ms 后才发最新帧,把一次 burst 合并成一张图。取代 rc.3 的条件去抖(仅对 >512 字节的输出去抖;小输出仍即时发送)。
- **滚动留 2 行**:上下翻页现在保留 2 行与上一屏的重叠(原为 1 行),提供更多上下文。

## [0.4.0-rc.3] - 2026-07-14

屏幕流量优化 + JPEG 质量档位。现在 screen 发送更克制,并可通过新参数在体积和质量间取舍。(中文版见 [`docs/CHANGELOG.zh-CN.md`](docs/CHANGELOG.zh-CN.md)。)

### 新增

- **JPEG 质量档位(`-q, --quality`)**:出图统一为 JPEG,可选 `high`(q85 彩色,默认)、`medium`(q70 彩色)、`low`(q50 黑白)。取代旧的 `-f` 图片格式参数。
- **screen 字节计数**:MQTT 出站任务每次 publish 时 log 累计 screen 字节数(MB),便于排查流量。

### 变更

- **更省流量**:screen 不再每次 PTY 输出都广播。启动时先等 PTY 静默满 500ms(上限 3s)再发第一帧 + presence;运行期单次大输出(>512 字节)触发 500ms 去抖,合并 burst、静默后发最新帧,小输出仍即时发送。

## [0.4.0-rc.2] - 2026-07-13

rc.1 的小幅跟进:新增 `skill` 子命令、修复 MQTT 重连、降低日志噪声,并全面整理了文档。

### 新增

- **`vibetty skill` 子命令**:把内置的 `run-vibetty` SKILL.md 安装进 / 移出 Claude Code 和 / 或 Codex 的用户级 skills 目录。版本感知(同版本跳过、版本不同则升级),uninstall 安全(绝不用 `remove_dir_all`,仅在目录随后为空时才删除)。
- **中英双语详细使用文档**:新增 `docs/USAGE.md`(英文)与 `docs/USAGE.zh.md`(中文),覆盖安装、配置、TUI、HTTP 端点、完整 MQTT 协议,以及 ESP32 / MCU 对接指南。

### 修复

- **MQTT 重连**:每次 ConnAck 都重新订阅入站 topic(`pty_in` / `control`),并在重连后补发 presence。此前重连可能出现「已连接却收不到消息」,且 presence 要等下一次 15s 心跳才恢复。
- **移除无用的 `PtyOutput` 广播路径**:浏览器前端已删,`PtyOutput` 无人消费,清掉了这条残留广播。

### 变更

- **日志更安静**:把高频的 WebSocket 事件日志从 `info` 降为 `debug`。
- **加大内部 channel**:broadcast / mpsc channel 容量 100 → 1024,避免负载高时丢消息。
- **文档整理**:按当前功能重写 README(删掉已移除的 ASR / 语音内容);删除独立的 `docs/esp32-mqtt-integration.md`(其协议内容已并入使用文档)。

---

## [0.4.0-rc.1] - 2026-07-13

首个引入 MQTT 通信方案的预发布版本。在保留 WebSocket(`/ws`)通道的同时,新增一条可选的 MQTT 通道,便于 ESP32/MCU 等不便运行 WebSocket 的设备接入;两者并存,复用同一个 PTY 会话。仅在配置文件含 `[mqtt]` 段时启用,否则行为与旧版一致。

### 新增

- **MQTT 传输通道**:在 WebSocket 之外新增第二条传输通道,与 WebSocket 并存,复用同一个 PTY 会话、`cli_tx` 与 broadcast `tx`;仅在配置含 `[mqtt]` 段时启用,否则完全不碰 MQTT。
- **MQTT 协议划分**:入站按键走独立 raw topic `pty_in`,控制类消息(输入文本 / 同步 / 滚动)合并到 `control`(payload 为 `ClientMessage` 的 JSON),出站发布整张 `screen` 图(无分块,格式靠 magic bytes 区分)。
- **内置 rumqttd broker**:可随启动自动起一个本地 broker(TCP + WS、匿名、1MB payload),也可改为连接自部署 broker 或免费云服务。
- **MQTT presence 在线公告**:在实例主题上 retained 公告在线状态,15 秒心跳;连接异常断开时由 LWT(空 retained)自动清理,无需手动下线。
- **多实例发现**:presence 主题前缀为 `{user}/{device}/{pid}/vibetty`,ESP32 订阅 `{user}/+/+/vibetty` 即可发现该用户名下的全部实例。
- **终端 agent 状态跟踪**:解析终端窗口标题,识别 Codex 与 Claude Code 的 working / waiting 状态。
- **agent 状态经 MQTT 广播**:working / waiting 状态随 presence 发布,ESP32 可据此判断是否需要把屏幕推送给用户。
- **TUI MqttPanel 弹窗**:点击 `MQTT` 按钮弹出面板,可手动起 / 停 client、启动内置 broker、编辑 broker URL 与端口(Enter 存回配置)。
- **TUI 顶部按钮行**:HTTP / MQTT / Fit / Quit 按钮从底部移到屏幕第一行;MQTT 按钮文字反映组合状态(`off` / `brkr` / `conn` / `on`)。
- **TUI 鼠标悬停高亮**:鼠标悬停时按钮高亮,通过 any-event 鼠标上报加两层节流避免刷屏。
- **Fit 按钮**:一键把终端尺寸重置为当前窗口尺寸(扣除按钮行与终端上边框后的可用区域)。
- **按需 HTTP 服务**:通过按钮启动 / 停止 HTTP server,无需常驻。
- **Quit 按钮**:在 TUI 中直接退出。
- **vibetty setup 配置 TUI**:`vibetty setup` 改为 ratatui 界面,可编辑 `[mqtt]` 的全部字段并写回配置文件(保留其它段)。
- **--config 启动参数**:新增 `--config` 覆盖默认的 `~/.vibetty/config.toml` 路径。

### 变更

- **通信方案**:WebSocket(`/ws`)保留为默认,MQTT 作为可选的第二通道并存;两者由同一个 PTY 会话驱动。
- **ASR 移至 ESP32**:语音识别改由 ESP32 本地完成,识别后的文本经 `control` 回传;服务端不再做转写与音频处理。
- **waiting 自动归位**:终端 agent 切到 waiting 状态时,自动把屏幕滚动重置到最新(scrollback = 0)。
- **终端截图默认格式**:默认输出 JPEG(可配置)。
- **MQTT broker 地址以配置为准**:client 连接的 broker URL 统一以配置中的 `broker` 为准,仅在 `broker` 为空且启用内置 broker 时才默认本地地址。

### 移除

- **浏览器前端**:删除 `resources/` 下的 `app.js`、`index.html`、`setup.html`、`vosk/` 等 Web 前端资源。
- **服务端 ASR**:删除 Whisper HTTP 与阿里云百炼实时 WS 转写、WAV / PCM 音频处理,及相关依赖(`wav_io`、`reqwest`、`reqwest-websocket`、`hanconv`)。
- **change-directory 功能**:移除旧的切换工作目录功能。
- **旧版 WebSocket 终端前端**:移除被取代的旧终端前端实现。

---

## 通信方案背景:为什么引入 MQTT

vibetty 现已在保留 WebSocket 的同时新增 MQTT 作为第二条通信通道。

现有的 WebSocket 方案有一个明显的局限:vibetty 运行在您的 PC 上,而 vibekeys 需要从外部网络访问它,这就必须把 vibetty 的端口映射到公网——要么使用端口映射服务,要么额外准备一台独立的服务器来做转发,既麻烦,也增加了成本与风险。

改用 MQTT 后,vibetty 和 vibekeys 都作为客户端连接到同一个 MQTT Broker,无需再把 PC 上的端口暴露出去:

- **可自行部署 Broker**:在您自己的服务器或设备上部署,所有通信数据都不经过任何第三方。
- **也可使用免费 MQTT 云服务**:若不想自行部署,注册一个免费的 MQTT 云服务(EMQX Cloud)即可使用,对单人使用场景完全足够。
- **可与信任的人共享 Broker**:同一个 Broker 可供信任的家人、朋友或同事一起接入,无需每人各自搭建或购买,进一步节省成本。

无论选择哪种方式,数据始终掌握在您自己手中,更加安全。

---

如需了解部署方式或有任何疑问,欢迎随时与我们联系。
