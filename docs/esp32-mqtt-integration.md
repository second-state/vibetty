# ESP32 端对接 vibetty MQTT 指南(给下一个 Claude)

> 本文档面向**在 ESP32 仓库工作的下一个 Claude**。目标:让 ESP32 通过 MQTT 连接运行在 PC 上的 vibetty(终端服务器),实现「发现 vibetty 实例 + 收发终端数据」。
>
> 你没有我们之前的对话上下文,所以本文把 vibetty 这边的 MQTT 协议**完整规格**列出来,作为 ESP32 必须遵守的契约。**协议规格以 vibetty 仓库 `src/mqtt.rs` 为准**,有疑问去读那个文件。

---

## 0. 一句话目标

ESP32 和 vibetty 连**同一个 MQTT broker**,通过 topic 通信:ESP32 发按键/控制给 vibetty,vibetty 回终端输出/屏幕截图。ESP32 还要先「发现」当前有哪些 vibetty 实例在线(因为实例的 topic 不能预知)。

---

## 1. vibetty 的 MQTT 协议规格(ESP32 必须遵守)

### 1.1 Broker 连接
- vibetty 是 MQTT 客户端,连一个外部 broker(用户自部署的 Mosquitto/EMQX,或免费 MQTT cloud)。
- **ESP32 必须连同一个 broker**(同样的 host/port)。
- 认证:用 broker 的 username/password(vibetty 的 `[mqtt] username`/`password`,见 `~/.vibetty/config.toml`)。ESP32 用同一个账号(或 broker 上另一个有权限的账号)。
- 端口:`1883` 明文 / `8883` TLS(8883 自动开 TLS)。

### 1.2 Topic 命名(关键 ⚠️)
每个 vibetty 实例的所有 topic 都在一个**自动构造的前缀**下:

```
{user}/{device}/{pid}/vibetty
```

| 段 | 来源 | 性质 |
|----|------|------|
| `user` | vibetty 配置的 `[mqtt] username`;**没配则 = `device`** | 稳定(用户配的) |
| `device` | `SHA256(machine-uid)` 前 16 hex(机器指纹) | 稳定(跨重启不变)+ 跨机器唯一 |
| `pid` | vibetty 进程 pid | **每次重启变** |

**为什么 ESP32 必须做 discovery**:`device` 是 PC 的机器指纹(ESP32 算不出)、`pid` 每次变(ESP32 无法预测)。所以 ESP32 **不能预知实例的 topic**,必须先通过 presence 发现。

### 1.3 Topic 清单
设 `P = {user}/{device}/{pid}/vibetty`(实例前缀):

| 方向 | topic | payload 格式 | 说明 |
|------|-------|------------|------|
| ESP32 → vibetty | `P/pty_in` | **raw bytes** | 原始按键字节(单键/转义序列) |
| ESP32 → vibetty | `P/control` | **JSON** | 控制消息(输入文本/同步/滚动) |
| vibetty → ESP32 | `P/pty_out` | **raw bytes** | PTY 终端输出字节流 |
| vibetty → ESP32 | `P/screen` | **raw bytes** | 整张 PNG 或 JPEG |
| vibetty → ESP32 | `P`(前缀本身) | **JSON, retained** | presence 公告(服务发现) |

### 1.4 control 的 JSON 格式
复用 vibetty 的 `ClientMessage` serde 形式(`#[serde(tag="type", content="data")]`),靠 `type` 区分。ESP32 只需发这 4 种:

| type | data | 含义 |
|------|------|------|
| `input_text` | 字符串 | 输入一段文本(如命令) |
| `sync` | `{width,height}` | `width`/`height` 是 ESP32 **显示区的像素**尺寸;服务端换算成列/行后 resize PTY 并刷新整屏 |
| `scroll_up` | (无) | 终端向上滚动 |
| `scroll_down` | (无) | 终端向下滚动 |

> `sync` 的尺寸单位是**像素**,不是终端的列/行。服务端按截图渲染参数换算:
> `cols = (width - 32) / 8`、`rows = (height - 32) / 18`(char cell 8×18px,四周留白各 16px,
> 见 `ws.rs` 的 `SCREEN_CHAR_WIDTH/HEIGHT`、`SCREEN_PADDING`)。ESP32 只要如实上报自己屏的像素即可。

示例:
```json
{"type":"input_text","data":"ls -la\n"}
{"type":"sync","data":{"width":320,"height":240}}
{"type":"scroll_up"}
```

> `pty_in`(raw 单键)和 `control` 的 `input_text`(文本串)的区别:单键/方向键/控制字符走 `pty_in` 的 raw 字节;整段文本/命令行走 `control` 的 `input_text`。

### 1.5 screen 的 payload
整张图片字节,**无分块、无信令字段**。ESP32 据前几个 magic bytes 判断格式:
- PNG:开头 `\x89\x50\x4e\x47`(即 `\x89PNG`)
- JPEG:开头 `\xff\xd8\xff`

然后丢给对应的解码器。

### 1.6 Discovery / presence 机制
- vibetty 上线时,在 `P`(前缀本身)发一条 **retained** 消息:
  ```json
  {"prefix":"alice/1a2b3c4d5e6f7a8b/12345/vibetty","client_id":"vibetty-12345","ts":1751300000}
  ```
  - `prefix` = 完整实例前缀(ESP32 据此订阅输出通道)
  - `client_id` = vibetty 的 MQTT client id(调试用)
  - `ts` = 当前 epoch 秒(ESP32 据此判活)
- **每 15s 重发一次**(心跳,刷新 ts)。
- **异常掉线**:broker 触发 LWT,向 `P` 发一条**空 payload**(= 删除 retained)。
- **正常退出**:目前**没有主动删**(靠 ts 超时兜底)。

**ESP32 的发现订阅**:
- 若 ESP32 知道 `user`(= 它自己连 broker 的 username,且 vibetty 也配了同一个):subscribe `{user}/+/+/vibetty`(`+` 通配 device 和 pid 两段)。
- 若不知道 user(vibetty 没配 username,user 段=device hash):用更宽的 `+/+/+/vibetty`(通配 user/device/pid 三段)。
- retained 保证 ESP32 一连上就**立即收到所有现存实例**的 presence。

---

## 2. ESP32 要实现的功能清单

1. ✅ 连接 broker(host/port/认证与 vibetty 配置一致)
2. ✅ **Discovery**:subscribe presence 通配 topic,解析 payload,维护「在线实例列表」
3. ✅ 选定目标实例后,subscribe `{P}/pty_out` + `{P}/screen`
4. ✅ 收 `pty_out` raw bytes → 渲染到 ESP32 显示(若有屏)/ 处理
5. ✅ 收 `screen` → 据 magic bytes 判断 PNG/JPEG → 解码显示
6. ✅ 发按键 → publish `{P}/pty_in`(raw bytes)
7. ✅ 发控制 → publish `{P}/control`(JSON)
8. ✅ **存活判断**:presence 的 `ts`(超过 ~30s 未更新当离线)+ LWT 空 payload(实例下线,立即移除)
9. ✅ **切换目标**:unsubscribe 旧实例的 `pty_out`/`screen`,subscribe 新实例的

---

## 3. 代码骨架(`esp-idf-svc`,以 0.52 为准)

用 `EspAsyncMqttClient`。API 细节以 `esp-idf-svc` 最新文档为准,下面是结构参考。

### 3.1 连接 + 发现订阅
```rust
use embedded_svc::mqtt::client::{AsyncClient, QoS};          // subscribe/unsubscribe/publish 来自这个 trait
use esp_idf_svc::mqtt::client::{EspAsyncMqttClient, MqttClientConfiguration};

let mut client = EspAsyncMqttClient::new(
    "mqtt://broker.example.com:1883",      // 或 mqtts://...:8883
    &mut MqttClientConfiguration {
        client_id: Some("vibetty-esp32-001"),   // broker 内必须唯一
        username: Some("alice"),                 // 与 vibetty 同一 broker 账号
        password: Some("secret"),
        buffer_size: 32 * 1024,                  // ⚠️ 必须配大,见 §4.1
        out_buffer_size: 8 * 1024,
        ..Default::default()
    },
)?;

// Discovery:订阅 presence(retained → 立即收到现存实例)
let user = "alice";   // ESP32 自己的 MQTT username;不确定就用 "+/+/+/vibetty"
client.subscribe(&format!("{user}/+/+/vibetty"), QoS::AtLeastOnce).await?;
```

### 3.2 收消息主循环
```rust
// state
let mut current_prefix: Option<String> = None;

loop {
    let msg = client.next().await?;
    let topic = msg.topic();        // &str
    let payload = msg.payload();    // &[u8]
    let segs: Vec<&str> = topic.split('/').collect();

    match segs.as_slice() {
        // presence: [user, device, pid, "vibetty"](4 段)
        [_, _, _, "vibetty"] => {
            if payload.is_empty() {
                // LWT:实例下线 → 清空当前目标
                current_prefix = None;
            } else {
                // {"prefix","client_id","ts"}
                let p: Presence = serde_json::from_slice(payload)?;
                if current_prefix.as_deref() != Some(&p.prefix) {
                    // 切换目标:先退订旧实例输出
                    if let Some(old) = current_prefix.take() {
                        client.unsubscribe(&format!("{old}/pty_out")).await?;
                        client.unsubscribe(&format!("{old}/screen")).await?;
                    }
                    current_prefix = Some(p.prefix.clone());
                    client.subscribe(&format!("{}/pty_out", p.prefix), QoS::AtLeastOnce).await?;
                    client.subscribe(&format!("{}/screen", p.prefix), QoS::AtLeastOnce).await?;
                }
            }
        }
        // 终端输出: [..., "vibetty", "pty_out"](5 段)
        [.., "vibetty", "pty_out"] => { /* raw bytes → 渲染 */ }
        // 屏幕截图: [..., "vibetty", "screen"]
        [.., "vibetty", "screen"]  => { /* 据 magic bytes 判 PNG/JPEG → 解码 */ }
        _ => {}
    }

    // 存活兜底:可另起定时器,若 current_prefix 对应实例的 ts 超过 30s 未更新,清空目标
}
```

### 3.3 发输入
```rust
// 单键 / raw 字节 → pty_in
client.publish(&format!("{prefix}/pty_in"), QoS::AtLeastOnce, false, &[b'a']).await?;

// 文本命令 → control(JSON)
client.publish(
    &format!("{prefix}/control"),
    QoS::AtLeastOnce, false,
    br#"{"type":"input_text","data":"ls -la\n"}"#,
).await?;
```

---

## 4. 关键细节 / 坑

1. **buffer 必须配大**:`screen` 是整张 PNG/JPEG(可能几十 KB),ESP-IDF mqtt 默认 `buffer_size` 不够会截断。`MqttClientConfiguration` 的 `buffer_size`(收)至少给 32KB,按实际截图大小调。`pty_out` 流量大时也要注意。
2. **二进制 OK**:`pty_in`/`pty_out`/`screen` 都是 raw bytes,esp-mqtt 支持 binary payload。
3. **retained 的正确处理**:首次 subscribe presence 时,broker 会把现存的 retained 一次性推过来(每个实例一条),所以 ESP32 一连上就有完整在线列表,不用等心跳。
4. **LWT 空 payload = 删除**:收到一条 payload **为空**的 presence 消息,就是实例下线信号,立即从列表移除。
5. **ts 判活兜底**:LWT 只在异常断连触发;正常退出靠 ts。ESP32 维护「最后见到的 ts」,**`now - ts > 30s`** 当离线(注意 ESP32 时钟要准,或用 broker 时间)。
6. **pid 跨重启变**:不要把 prefix 持久化缓存,每次启动重新 discovery。
7. **unsubscribe 是异步的**:`unsubscribe().await` 返回时只是包已发,broker ACK 前可能还收几条该 topic 的消息,要能容忍(忽略已退订 topic)。
8. **TLS**:8883 用 `mqtts://`;若 ESP32 走浏览器式 wss 不适用(ESP32 直连 TCP MQTT,不用 WS)。
9. **username 与 user 段**:ESP32 连 broker 的 username **就是** vibetty 的 `user` 段(前提:vibetty 配了 `[mqtt] username` 且账号一致)。若 vibetty 没配 username(user=device hash),ESP32 不知道 user 段,只能宽通配 `+/+/+/vibetty`。

---

## 5. 待你确认 / 调查的 ESP32 侧未知项

下一个 Claude 接手时,这些需要先摸清(本仓库看不到 ESP32 代码):

- **ESP32 项目结构**:是基于现有代码改,还是从零搭?用的 `esp-idf-svc` 版本?
- **ESP32 有没有屏幕**:`screen` 截图是否需要解码显示?没有屏的话,screen 可以不订阅(省带宽),只走 `pty_out` raw + 自己渲染。
- **ESP32 怎么拿 broker 配置**:hardcode?配网?和 vibetty 共享一份配置?
- **`user` 段的确定**:vibetty 那台是否配了 `[mqtt] username`?决定 ESP32 用 `{user}/+/+/vibetty` 还是 `+/+/+/vibetty`。
- **JSON 库**:ESP32 是 std(esp-idf)还是 no_std?std 可用 `serde_json`;no_std 要用 `serde-json-core` 或手写 JSON(control 消息很简单,手写也行)。

---

## 6. 验证方法(不依赖 ESP32 也能测)

先在本地把 vibetty 侧跑通,再用 Python 模拟 ESP32 验证协议:

1. **本地 broker**:
   ```bash
   mosquitto -c /tmp/mosq.conf   # 最小配置:listener 1883 127.0.0.1 + allow_anonymous true
   ```
2. **看 vibetty 的 presence**:
   ```bash
   mosquitto_sub -t '+/+/+/vibetty' -c -v
   # 应立即看到一条 retained JSON,每 15s 刷新 ts
   ```
3. **Python paho-mqtt 模拟 ESP32**(CLI 在本机 sandbox 下 mosquitto_pub/sub 可能报 bad fd,用 python 更顺):
   ```python
   import paho.mqtt.client as mqtt, json, time
   c = mqtt.Client()
   c.connect("127.0.0.1", 1883)
   # 发现
   def on_msg(_, __, m):
       print(m.topic, m.payload[:80])
   c.on_message = on_msg
   c.subscribe("+/+/+/vibetty")
   c.loop_start(); time.sleep(2)
   # 发按键(把 <prefix> 换成上一步看到的值)
   prefix = "alice/1a2b3c4d5e6f7a8b/12345/vibetty"
   c.publish(f"{prefix}/pty_in", b"l")
   c.publish(f"{prefix}/control", json.dumps({"type":"input_text","data":"s\n"}))
   time.sleep(2)
   ```
   vibetty 的终端应出现 `l` 和 `s` 命令的输出。
4. **测 LWT**:kill vibetty,看 `mosquitto_sub` 是否收到一条空 payload(presence 被删)。

---

## 7. vibetty 侧相关文件(核对协议用)

- `src/mqtt.rs` — MQTT 桥接:topic 构造(`instance_prefix`)、presence(`presence_payload`)、LWT、心跳、`parse_control`(control JSON 解析)、`INBOUND_TOPICS`。
- `src/config.rs` — `MqttConfig`(host/port/username/password/qos/keep_alive 等)、`effective_client_id()`、`effective_use_tls()`。
- `src/protocol.rs` — `ClientMessage` 枚举(control JSON 的 serde 来源:input_text/sync/scroll_up/scroll_down 等)。

> 协议有任何不确定,**以 `src/mqtt.rs` 实际代码为准**,本文是它的快照。
