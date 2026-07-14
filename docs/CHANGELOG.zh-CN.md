# 变更日志 (CHANGELOG)

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
