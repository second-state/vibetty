# vibetty — 项目备忘

axum 0.8 WebSocket 终端服务器。把一个 PTY 会话同时当成「浏览器前端」(WebSocket `/ws`)和「终端截图生成器」用。edition 2024,ratatui/crossterm TUI,vt100 终端模拟,portable-pty。

## 工作约定

- **改完 Rust 代码、提交/推送前先 `cargo fmt`**。CI(`.github/workflows/`)跑 `cargo fmt --check` + clippy + build(ubuntu+windows),没格式化直接挂 CI。
  - `cargo fmt --manifest-path` 的值要指向 **`Cargo.toml` 文件**,传目录会报错。
- CI 对纯 markdown/docs 改动会跳过(`bd6508f` 之后),但代码改动照常全跑。

---

## 终端截图:调色板 PNG(已上线 main,`dd4331b`)

`ws.rs:968` 的 PNG 分支已从「image crate 默认编码」换成 `png_encode::encode_paletted_png`,终端截图体积 ~82.5K → ~22K(PSNR ~49dB)。JPEG 质量从 100 调到 85(`ws.rs:982`)。

**为什么不用 image crate 默认 PNG**:默认 `PngEncoder` 压缩很弱,同样的调色板图 image crate 出 ~77.5K,而 png crate + `Compression::Best` 能压到 ~22K。

**`src/png_encode.rs` 的非显而易见点**:
- NeuQuant 必须用 **RGBA(4 字节/像素)** 训练,用 RGB 会让索引错位、颜色全乱(PSNR 掉到 26dB)。训练完 `color_map_rgb()` 拿调色板。
- `index_of` 入参也是 RGBA 4 字节,不能传 RGB。
- png crate 写 8-bit indexed 时 `ColorType::Indexed` + 一次性 `write_image`,配合 `Compression::Best`。

---

## 可选 MQTT 传输(feat/mqtt-transport 分支)

给 ESP32/MCU 这类不方便跑 WS 的设备加第二条传输通道。**配置驱动、可选**:只在 `~/.vibetty/config.toml` 有 `[mqtt]` 段时才启用;否则完全不碰 MQTT,WebSocket/HTTP 原样保留。两条通道并存,复用同一个 PTY 会话、`cli_tx` / broadcast `tx`、PTY 逻辑。

### 配置(`MqttConfig`,config.rs)

```toml
[mqtt]
enable = true           # transport client 的 auto-start 开关
broker = ""             # mqtt(s)://[user:pass@]host:port;空 + builtin_broker 时默认本地
builtin_broker = true   # 内置 broker(rumqttd)的 auto-start 开关
builtin_port = 1883     # 内置 broker TCP 端口
builtin_ws_port = 9001  # 内置 broker WS 端口
qos = 1                 # ⚠️ 当前未生效:inbound QoS 在 mqtt.rs 写死(pty_in=0, control=1);此字段只在 setup TUI 可编辑
keep_alive_secs = 30
```

`enable` / `builtin_broker` 现在纯粹是 **auto-start 标志**:boot 时 `run_command` 见 `enable`→起 transport client、`builtin_broker`→起内置 broker。两者还能在 TUI 弹窗里手动起/停。

**URL 解析(`MqttConfig::for_client`,config.rs)**:client 连的 broker URL 一律以 config 里 `broker` 为准;**只有** `broker` 空 **且** `builtin_broker=true` 时才默认填本地 `mqtt://127.0.0.1:{builtin_port}`。即:即便内置 broker 开着,只要 `broker` 填了,client 就连填的地址。boot 自动起、面板(重)spawn、URL 预填/比对都走这一处(唯一真相源)。

### TUI 控制(footer MQTT 按钮)

footer 是 `HTTP | MQTT | Quit`;`[mqtt]` 配置存在即显示 `MQTT` 按钮(不再绑 `builtin_broker`),点击弹 **MqttPanel**(上下两块):
- **Broker 块**:`TCP:` / `WS :` 端口可编辑(Enter 存回 config)+ `Start broker`(起内置 rumqttd;**只能起不能停**——rumqttd 无 shutdown API,起了之后变只读 `● broker running :{port}`)。
- **Client 块**:`URL:` broker 地址可编辑(Enter 存回 `[mqtt] broker`)+ `Start client`/`Stop client`。
- `Tab`/`↑↓` 在 `TCP → WS → BrokerStart → URL → ClientToggle` 循环;Enter 行为随聚焦项变(端口=存盘、BrokerStart=起 broker、URL=存 URL、ClientToggle=切 client);底部提示也随聚焦项动态变化。
- footer MQTT 按钮文字反映组合状态:`MQTT off` / `MQTT brkr` / `MQTT conn` / `MQTT on`(brkr=broker 在跑 client 没跑,以此类推)。

### footer 渲染与悬停高亮(ui/mod.rs + ws.rs,`9cb9c69`)

footer 按钮的**外观与鼠标交互**(按钮**功能**见上节):
- footer 是 `Layout::Vertical` 第三段 `Length(1)`——**单行**,不是以前带边框的 3 行 block。`TUI_ROWS_PADDING=6` = header 3 + footer 1 + 终端 pane 上下边框各 1。
- 按钮**无边框**:`render_button` 用 `Paragraph`+`Style.bg` 整块填色(默认 `DarkGray`,悬停 `LightBlue`+黑字),不是 `Block::borders`(`borders` 会让按钮高 3 行)。
- 按钮**左右各内缩 1 格**对齐 header **内容区**:header 是带边框 block、内容在边框内 1 格;按钮去边框后若从 `footer.x` 起会落到 header 边框列,视觉错位。故 `http_button_rect`=`footer.x+1`、`quit_button_rect`=`footer.x+footer.width-1-width`;MQTT 按钮从 HTTP 右侧推导、自动跟随。
- **命中 rect 三处共用**:`render_frame` 渲染、`handle_click` 点击、`footer_button_at` 悬停命中都走 `*_button_rect`/同一 `Layout`——改按钮布局要同步这几处。
- **悬停高亮**靠 crossterm `?1003h`(any-event,`Moved` 无需按键上报),两层节流避免刷屏:
  1. producer 侧(`event_loop_thread`):维护 `last_row`(init `terminal::size()`、Resize 同步),`Moved` 只在 `row==last_row`(footer 行)或刚离开 footer 行时转发;终端区滑动直接丢、不进 channel。
  2. consumer 侧(`run_command` Hover 分支):`footer_button_at` 算当前悬停按钮,只在变化时重绘。
  - `hover`(`HoveredBtn`,Copy)是普通 `let mut` 变量、当参数传进 `redraw` 闭包。

### client 可中断/重启(oneshot cancel)

`mqtt::spawn` 返回 `MqttHandle{cancel: oneshot::Sender<()>}`;`run_bridge` 主循环**一个 `select!` 同时跑三路**:cancel、入站 `eventloop.poll()`、出站 `rx.recv()`(收 `Screen` 渲染补齐后 publish)。`MqttHandle::stop()` 发 cancel → break → `client.disconnect()` + `abort()` 心跳任务(出站已并入主 select,随 break 一同停,无需单独 abort;否则心跳会一直往死连接上发、刷 warn)。真正的下线清理靠 **LWT**(空 retained):连接一断 broker 自动清 presence。**停 client 期间的消息不会缓存、重启不补发**(broadcast 只投递给当前订阅者)。

### 协议(用户拍板)

**不用 msgpack**。原始按键走独立 raw topic(`pty_in`);控制类消息(输入文本/同步/滚动)合并到一个 `control` topic,payload 是 `ClientMessage` 的 serde JSON(`{"type":...,"data":...}`)。出站只发 `Screen`(整张 PNG/JPEG);`pty_out` 当前**调试期停发**(只 log);`notification`/`title` 不走 MQTT。

Topic 前缀 `{user}/{device}/{pid}/vibetty`:`device`=SHA256(machine-uid) 前 16 hex、`pid`=进程 pid、`user`=`username`(空则 `root`)。
- 入站(订阅):`{p}/pty_in`(raw→PtyInput)、`{p}/control`(JSON→`parse_control`,只收 input/sync/scroll_*)。
- 出站:`{p}/screen`(整张图←Screen,无分块,格式靠 magic bytes 区分)。
- presence:`{p}` 上 retained 公告,15s 心跳;LWT 异常掉线清空。ESP32 订阅 `{user}/+/+/vibetty` 发现该用户所有实例。

### 改动要点

- `mqtt.rs`:`spawn()->MqttHandle` + `run_bridge(cfg, cli_tx, tx, fmt, cancel)`。出/入站都在主 `select!` 里:出站 `rx.recv()` 分支(收 `Screen`→`render_screen_to_image` 补齐到 sync 尺寸→publish)、入站 `eventloop.poll()`→strip 前缀→`pty_in`/`control`→`cli_tx`、外加 cancel 分支。sync 尺寸是普通 `u16` 局部变量(出/入站同任务顺序访问,不再需要 `Arc<AtomicU16>`)。退出时 disconnect + abort 心跳。
- `config.rs`:`MqttConfig` 见上;`for_client()`(URL 解析,唯一一处)。`RunArgs::mqtt_config()` 固定从 `~/.vibetty/config.toml` 读 `[mqtt]`。
- `main.rs`:**不再 boot 起 transport**;只 `args.mqtt_config()` 拿 `mqtt_cfg` + `cli_tx` 传给 `run_command`。
- `ws.rs`:`run_command` 负责 boot 自动起(client if enable、broker if builtin_broker)+ footer 按钮起停 + MqttPanel 弹窗;新增 `cli_tx` 形参(运行期重 spawn client 用)。`parse_and_save` 同步更新内存 `mqtt_cfg`(避免改端口后重启 client 连旧端口)。`render_screen_to_image` 是 `pub(crate)` 供 mqtt.rs 用。
- `setup.rs`:`vibetty setup` 是 ratatui TUI,编辑 `[mqtt]` 全部字段写回 config;`save_mqtt` 是 `pub(crate)`(ws.rs 写 config 复用),用 `toml::Table` 保留其它段。
- broker.rs:`spawn_builtin(&cfg)` 在独立 OS 线程跑 rumqttd(TCP + WS,匿名,1MB payload)。**无 shutdown**。

### 验证状态

- 编译 / clippy `-D warnings` / `cargo fmt --check` / 单测全绿。
- 入站 `pty_in` 早先端到端验过(本地 mosquitto + paho-mqtt)。`control`、出站 `screen`、client stop/restart(oneshot + LWT 清 presence)、URL 编辑存盘、面板起停 broker **待端到端重验**。
- headless 出站:80×24 默认尺寸已兜底(不 panic),但要收到 `screen` 仍需 PTY 有输出触发广播。
- 出站曾卡 headless vt80 panic(无 TTY 尺寸 0×0),已修:`ws.rs` 拿不到有效尺寸默认 80×24(`f160599`)。

---

## ASR 已从服务端移除(`6d5e4b3`)

ASR 改由 **ESP32 本地**完成:ESP32 识别后把**文本**经 `{p}/control` 发回 vibetty(省音频流量)。vibetty 服务端不再做转写/音频处理。

- 删:`src/asr/`(Whisper HTTP + 阿里云百炼实时 WS)、`src/util.rs`(WAV/PCM 音频处理)、依赖 `wav_io`/`reqwest`/`reqwest-websocket`/`hanconv`;以及 `ASRInterface`、`check_deprecated_env_vars`、`check_vosk_models`、`/vosk_ws` 端点。
- **保留变体(关键)**:`protocol.rs` 一行没动——`ClientMessage::VoiceInputStart/Chunk/End`、`ServerMessage::AsrResult` 都还在。浏览器前端 `index.html`/`app.js` 仍发 voice,后端 `ws.rs` 收到后在一个合并的 match arm **静默丢弃**(不转写、不回 `asr_result`),WS 不会因反序列化失败断连。前端 voice UI 由用户后续清理。
- `vibetty setup` 从「配 ASR」改成「配 MQTT」TUI(见上节)。

---

## 调试/运行备忘

- 无配置回归:`~/.vibetty/config.toml` 不写 `[mqtt]`,启动 vibetty 应该零 MQTT 日志、WS 行为不变。
- 本地 broker:`mosquitto -c /tmp/mosq.conf`(我用的最小配置:`listener 1883 127.0.0.1` + `allow_anonymous true`)。注意 `mosquitto_pub/sub` CLI 在本机 sandbox 下会报 "Bad file descriptor",用 Python `paho-mqtt` 代替更顺。
- headless 下验出站:80×24 默认尺寸已兜底(不会再 panic),但要收到 `pty_out`/`screen` 仍需 PTY 有输出触发广播。
- 日志:flexi_logger 写到 CWD 的文件,不是 stdout——跑完去 CWD 看 `vibetty*.log`。
