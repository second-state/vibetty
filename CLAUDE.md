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

## 可选 MQTT 传输(feat/mqtt-transport 分支,**working tree 未 commit**)

给 ESP32/MCU 这类不方便跑 WS 的设备加第二条传输通道。**配置驱动、可选**:只在 `~/.vibetty/config.toml` 有 `[mqtt]` 段时才连外部 broker;没配置就完全不碰 MQTT,WebSocket/HTTP 原样保留。两条通道并存,复用同一个 PTY 会话、`cli_tx` / broadcast `tx`、PTY 逻辑,**零改动**。

**协议设计(用户拍板)**:**不用 msgpack**。原始按键走独立 raw 字节 topic(`pty_in`);其余 5 个控制类消息(输入文本/切目录/同步/滚动)合并到一个 `control` topic,payload 是 `ClientMessage` 的 serde JSON(`{"type":...,"data":...}`),靠 `type` 字段区分——ESP32 只解一次 JSON,且 `type` 命名和 WS 协议一致。只桥接终端核心消息(`notification`/`asr_result`/`title`/`voice_*` 不走 MQTT)。

Topic 约定(`{topic_prefix}` 来自 `[mqtt]`,默认 `vibetty`):
- 入站(ESP32→vibetty,vibetty 订阅):`{p}/pty_in`(原始按键字节→PtyInput)、`{p}/control`(JSON `{"type":...}`,覆盖 input_text/change_dir/sync/scroll_up/scroll_down,见 `mqtt.rs::parse_control`)
- 出站(vibetty→ESP32):`{p}/pty_out`(PTY 原始输出←PtyOutput)、`{p}/screen`(整张 PNG/JPEG 字节←Screen,无分块,格式靠 magic bytes 区分)

改动文件:
- `src/mqtt.rs`(新增):`spawn()` + `run_bridge()`。出站任务订阅 broadcast,只转发 `PtyOutput` 和 `Screen`(`Screen`→`ws::render_screen_to_image` 渲染整张图后发布,无分块);入站循环 `eventloop.poll()`→strip topic 前缀→`pty_in` 直构造 `PtyInput`、`control` 走 `parse_control()`(serde JSON,只接受 input/change_dir/sync/scroll_*,其余告警丢弃)→`cli_tx`。`poll()` 出错只 warn+sleep 2s(rumqttc 自动重连,WS/PTY 不受影响)。
- `src/config.rs`:`MqttConfig{host,port=1883,client_id,use_tls:Option<bool>,username,password,topic_prefix="vibetty",qos=1,keep_alive_secs=30}` + `effective_use_tls()`(port==8883 自动开)、`effective_client_id()`(空则 `vibetty-{pid}`);`RunArgs::mqtt_config()->Option<MqttConfig>` 解析 `[mqtt]` 段,没有就 `None`。
- `src/main.rs`:解析到 `MqttConfig` 就 `mqtt::spawn(...)`;`None` 整段跳过。
- `src/ws.rs`:纯可见性改动——`render_screen_to_image` 和 `IMAGE_CHUNK_SIZE` 改 `pub(crate)`。
- `Cargo.toml`:`rumqttc = "0.25.1"`(+ 之前的 `color_quant`/`png`)。

**验证状态**:
- 入站 `pty_in` 已验证(本地 mosquitto + python paho-mqtt 发 `pty_in`→vibetty 收到→`cli_tx`)。`control`(JSON 合并)是新设计,待重验。
- 出站之前卡在 headless vt80 panic(`grid.rs:26 "attempt to subtract with overflow"`)——根因是无 TTY 时 `crossterm::terminal::size()` 返回 0×0。**已修**:拿不到有效尺寸时默认 80×24(`ws.rs`,commit `f160599`)。出端端到端(本地 broker 收到 `pty_out`/`screen`)仍待验。

`config.toml` 示例:
```toml
[mqtt]
host = "127.0.0.1"     # 或 EMQX/Mosquitto 地址
port = 1883            # 8883 自动开 TLS
topic_prefix = "vibetty/test"
qos = 1
# client_id / username / password / use_tls 可选
```

---

## 调试/运行备忘

- 无配置回归:`~/.vibetty/config.toml` 不写 `[mqtt]`,启动 vibetty 应该零 MQTT 日志、WS 行为不变。
- 本地 broker:`mosquitto -c /tmp/mosq.conf`(我用的最小配置:`listener 1883 127.0.0.1` + `allow_anonymous true`)。注意 `mosquitto_pub/sub` CLI 在本机 sandbox 下会报 "Bad file descriptor",用 Python `paho-mqtt` 代替更顺。
- headless 下验出站:80×24 默认尺寸已兜底(不会再 panic),但要收到 `pty_out`/`screen` 仍需 PTY 有输出触发广播。
- 日志:flexi_logger 写到 CWD 的文件,不是 stdout——跑完去 CWD 看 `vibetty*.log`。
