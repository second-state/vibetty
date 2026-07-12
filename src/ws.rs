use std::net::SocketAddr;
use std::sync::Arc;

use axum::{Router, extract::State, response::IntoResponse, routing::get};
use ratatui::layout::Rect;
use tokio::sync::{broadcast, mpsc};
use tui_input::InputRequest;
use vt100::Callbacks;

use crate::broker;
use crate::config::MqttConfig;
use crate::mqtt;
use crate::protocol::{ClientMessage, ServerMessage};
use crate::ui::{
    HoveredBtn, HttpBtnState, ModalState, MqttButtonsState, MqttFocus, button_label, button_row_at,
    hit_test, http_button_rect, mqtt_button_label, mqtt_button_rect, quit_button_rect,
};

/// Image broadcast frame interval
const IMAGE_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// 终端截图里单个字符单元格的像素宽。`vibetty-screenshot` 在 font_size=14.0 +
/// 内嵌字体 Sarasa Mono SC Light(swash 后端)下由 `get_char_metrics(14.0)` 实测得到。
/// 客户端 Sync 发的是【像素】,要除以它换算成 PTY 列数。
/// ⚠️ 改 `render_screen_to_image` 的 font_size 或换字体时必须同步更新。
pub(crate) const SCREEN_CHAR_WIDTH: u32 = 8;
/// 单个字符单元格的像素高,同上(`get_char_metrics(14.0)` 返回 `(8, 18)`)。
/// 用于由 sync 的像素高度换算「一页」的行数。
pub(crate) const SCREEN_CHAR_HEIGHT: u32 = 18;
/// 截图四周的留白(每边像素数),来自 `ScreenshotConfig::default().padding`。
/// 整张图 = cols×`SCREEN_CHAR_WIDTH` + 2×`SCREEN_PADDING` 宽、
///         rows×`SCREEN_CHAR_HEIGHT` + 2×`SCREEN_PADDING` 高。
pub(crate) const SCREEN_PADDING: u32 = 16;
/// Columns reserved for TUI decorations: the terminal pane has no left/right borders
/// (only top + bottom), so nothing is reserved horizontally.
const TUI_COLS_PADDING: u16 = 0;
/// Rows reserved for TUI decorations: top button row (1) + the terminal pane's top border
/// (1 row; no bottom border). (Header 标题块已移除。)
const TUI_ROWS_PADDING: u16 = 2;

type ServerTx = broadcast::Sender<ServerMessage>;

type ClientRx = mpsc::Receiver<ClientMessage>;

struct WindowCallbacks {
    title: String,
    update_title: bool,
}

impl WindowCallbacks {
    fn new() -> Self {
        Self {
            title: String::new(),
            update_title: false,
        }
    }
}

impl Callbacks for WindowCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = std::str::from_utf8(title).unwrap().to_string();
        self.update_title = true;
    }
}

/// Screenshot request: send a oneshot sender to get the rendered JPEG bytes
type ScreenshotTx = mpsc::Sender<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>;

#[derive(Clone)]
pub struct AppState {
    pub screenshot_tx: ScreenshotTx,
    pub image_format: crate::protocol::ImageFormat,
}

fn send_screen(tx: &ServerTx, screen: Arc<vt100::Screen>) {
    let _ = tx.send(ServerMessage::Screen(screen));
}

/// 构造 HTTP 路由:/mqtt_ws 调试页 + /screenshot(与原启动期一致)。
fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/mqtt_ws", get(crate::static_page::mqtt_ws_handler))
        .route("/screenshot", get(screenshot_handler))
        .with_state(state)
}

/// 按指定绑定地址(如 `0.0.0.0:3000`、`127.0.0.1:8080`)后台启动 HTTP server,
/// 返回实际绑定的 `host:port`。失败(地址占用/无权限/格式非法等)返回 Err。
async fn start_http(bind: &str, state: AppState) -> anyhow::Result<String> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let bound = listener.local_addr()?.to_string();
    let app = build_router(state);
    tokio::spawn(async move {
        let serve = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        );
        if let Err(e) = serve.await {
            log::error!("[http] server error: {e}");
        }
    });
    Ok(bound)
}

/// 解析两个端口(须 > 0);任一与 config 不同则把整段 `[mqtt]` 写回 `~/.vibetty/config.toml`,
/// 并同步更新内存里的 `mqtt_cfg`(后续 client (重)spawn 的 `for_client()` 用得到——
/// 避免「面板改端口后重启 client 仍连旧端口」的不一致)。
fn parse_and_save(
    tcp: &tui_input::Input,
    ws: &tui_input::Input,
    mqtt_cfg: &mut Option<MqttConfig>,
) -> Result<(u16, u16), String> {
    let t = tcp
        .value()
        .trim()
        .parse::<u16>()
        .map_err(|_| "invalid TCP port".to_string())?;
    let w = ws
        .value()
        .trim()
        .parse::<u16>()
        .map_err(|_| "invalid WS port".to_string())?;
    if t == 0 || w == 0 {
        return Err("port must be > 0".to_string());
    }
    if let Some(cfg) = mqtt_cfg
        && (cfg.builtin_port != t || cfg.builtin_ws_port != w)
    {
        cfg.builtin_port = t;
        cfg.builtin_ws_port = w;
        crate::setup::save_mqtt(cfg, None).map_err(|e| format!("save config failed: {e}"))?;
        log::info!("[mqtt] saved builtin_port={t}, builtin_ws_port={w}");
    }
    Ok((t, w))
}

/// 滚动行数换算:`rows == 0` 表示滚一整页(= `page_rows`),否则滚 `rows` 行。
fn scroll_delta(rows: u16, page_rows: u16) -> usize {
    if rows == 0 {
        page_rows as usize
    } else {
        rows as usize
    }
}

/// 由客户端 sync 发的像素高度算「一页」的行数:能塞下的行数 − 1(滚动时留一行可见)。
fn page_rows_from_height(height: u16) -> u16 {
    let fits = (height as u32).saturating_sub(2 * SCREEN_PADDING) / SCREEN_CHAR_HEIGHT;
    fits.saturating_sub(1).max(1) as u16
}

/// 主事件循环统一的事件来源(PTY 输出 / 客户端消息 / TUI UI 事件 / 截图请求)。
enum TerminalEvent {
    Input(crate::protocol::ClientMessage),
    InputClosed,
    UIEvent(crate::ui::UIEvent),
    PtyOutput(String),
    ScreenGetter(tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>),
    Error,
}

/// 点击 footer 按钮的结果:退出程序 / 打开对话框 / 无命中。
/// `Modal` 装箱:另两个变体无数据,装箱避免整个枚举撑到 ModalState 的大小
/// (clippy::large_enum_variant);该值每次点击产生后立即消费,一次分配可忽略。
enum ClickOutcome {
    Quit,
    Modal(Box<ModalState>),
    None,
}

/// MqttPanel 操作需要的可变状态(改端口/URL 写 config、起停 broker/client)。
/// 把这几样 `&mut` 收进一个结构体,免去面板处理函数一长串参数。
struct MqttCtx<'a> {
    cfg: &'a mut Option<MqttConfig>,
    broker_on: &'a mut bool,
    client: &'a mut Option<mqtt::MqttHandle>,
    cli_tx: &'a mpsc::Sender<ClientMessage>,
    tx: &'a ServerTx,
    image_format: crate::protocol::ImageFormat,
}

/// 把原始按键解析成对 tui_input 的编辑请求:方向键/Home/End 导航、Backspace 删除、
/// 单字符插入(由 `allowed` 过滤、且当前长度 `< cap` 时才插)。端口/地址/URL 输入框共用,
/// 消除原先散落各处的重复 if 链。
fn parse_input_request(
    bytes: &[u8],
    allowed: impl Fn(u8) -> bool,
    cur_len: usize,
    cap: usize,
) -> Option<InputRequest> {
    if bytes == b"\x1b[D" {
        Some(InputRequest::GoToPrevChar)
    } else if bytes == b"\x1b[C" {
        Some(InputRequest::GoToNextChar)
    } else if bytes == b"\x1b[H" {
        Some(InputRequest::GoToStart)
    } else if bytes == b"\x1b[F" {
        Some(InputRequest::GoToEnd)
    } else if bytes == [0x08] || bytes == [0x7f] {
        Some(InputRequest::DeletePrevChar)
    } else if bytes.len() == 1 && cur_len < cap && allowed(bytes[0]) {
        Some(InputRequest::InsertChar(bytes[0] as char))
    } else {
        None
    }
}

/// MQTT broker URL 输入框允许的字符(字母/数字 + URL 常见符号)。
fn url_char_allowed(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b':' | b'/' | b'.' | b'_' | b'-' | b'~' | b'@')
}

/// boot 时的 MQTT auto-start:`enable` 起 client、`builtin_broker` 起内置 broker。
/// 返回 (broker 是否在跑, client handle)。broker 只起不停(rumqttd 无 shutdown)。
fn autostart_mqtt(
    mqtt_cfg: &Option<MqttConfig>,
    cli_tx: &mpsc::Sender<ClientMessage>,
    tx: &ServerTx,
    image_format: crate::protocol::ImageFormat,
) -> (bool, Option<mqtt::MqttHandle>) {
    let Some(cfg) = mqtt_cfg else {
        return (false, None);
    };
    let mut broker_on = false;
    if cfg.builtin_broker {
        match broker::spawn_builtin(cfg) {
            Ok(()) => {
                broker_on = true;
                log::info!("[mqtt] broker auto-started on :{}", cfg.builtin_port);
            }
            Err(e) => log::warn!("[mqtt] broker auto-start failed: {e}"),
        }
    }
    let client = cfg.enable.then(|| {
        log::info!("[mqtt] client auto-started");
        mqtt::spawn(cfg.for_client(), cli_tx.clone(), tx.clone(), image_format)
    });
    (broker_on, client)
}

/// 处理 HTTP 端口输入框的按键:Enter 按地址起 server、Esc 取消、其余编辑输入。
async fn handle_port_input(
    mut input: tui_input::Input,
    bytes: &[u8],
    http: &mut HttpBtnState,
    screenshot_tx: &ScreenshotTx,
    image_format: crate::protocol::ImageFormat,
) -> ModalState {
    // Enter:尝试按输入地址起 HTTP server。
    if bytes == b"\r" || bytes == b"\n" {
        let bind = input.value().trim().to_string();
        if bind.is_empty() {
            return ModalState::Error("invalid address".to_string());
        }
        let state = AppState {
            screenshot_tx: screenshot_tx.clone(),
            image_format,
        };
        return match start_http(&bind, state).await {
            Ok(addr) => {
                log::info!("[http] started on {addr}");
                *http = HttpBtnState::On(addr);
                ModalState::None
            }
            Err(e) => {
                log::warn!("[http] start failed: {e}");
                ModalState::Error(format!("listen failed: {e}"))
            }
        };
    }
    if bytes == b"\x1b" {
        return ModalState::None; // Esc 取消
    }
    // 地址可含字母/数字/.:[];导航/Home/End/Backspace 通用。
    let req = parse_input_request(
        bytes,
        |c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b':' | b'[' | b']'),
        input.value().len(),
        45,
    );
    if let Some(req) = req {
        input.handle(req);
    }
    ModalState::PortInput { input }
}

/// 把面板表单重新打包(保留输入字段、更新聚焦项)。
fn mqtt_panel(
    tcp: tui_input::Input,
    ws: tui_input::Input,
    url: tui_input::Input,
    focus: MqttFocus,
) -> ModalState {
    ModalState::MqttPanel {
        tcp,
        ws,
        url,
        focus,
    }
}

/// MqttPanel 上按 Enter:行为随聚焦项不同。
///  Tcp/Ws       -> 解析存盘(有改动则写 config)+ 跳到 BrokerStart。
///  BrokerStart  -> 起 broker(已起则无动作,rumqttd 停不掉)。
///  Url          -> broker URL 改动了则写回 [mqtt] broker + 跳到 ClientToggle。
///  ClientToggle -> 切换 client:已起则 stop(oneshot),否则 spawn。
async fn mqtt_panel_enter(
    tcp: tui_input::Input,
    ws: tui_input::Input,
    url: tui_input::Input,
    mut focus: MqttFocus,
    mqtt: &mut MqttCtx<'_>,
) -> ModalState {
    match focus {
        MqttFocus::Tcp | MqttFocus::Ws => match parse_and_save(&tcp, &ws, mqtt.cfg) {
            Ok(_) => {
                focus = MqttFocus::BrokerStart;
                mqtt_panel(tcp, ws, url, focus)
            }
            Err(e) => ModalState::Error(e),
        },
        MqttFocus::BrokerStart => {
            if *mqtt.broker_on {
                // 已起,rumqttd 停不掉 -> 无动作,留在面板。
                mqtt_panel(tcp, ws, url, focus)
            } else {
                match parse_and_save(&tcp, &ws, mqtt.cfg) {
                    Ok((t, w)) => match tokio::net::TcpListener::bind(("0.0.0.0", t)).await {
                        Ok(_) => {
                            let mut cfg = mqtt
                                .cfg
                                .as_ref()
                                .expect("mqtt_cfg_present implies mqtt_cfg")
                                .clone();
                            cfg.builtin_port = t;
                            cfg.builtin_ws_port = w;
                            match broker::spawn_builtin(&cfg) {
                                Ok(()) => {
                                    log::info!("[mqtt] broker started on :{t}");
                                    *mqtt.broker_on = true;
                                    ModalState::None
                                }
                                Err(e) => ModalState::Error(format!("broker start failed: {e}")),
                            }
                        }
                        Err(e) => {
                            log::warn!("[mqtt] port {t} unavailable: {e}");
                            ModalState::Error(format!("port {t} unavailable: {e}"))
                        }
                    },
                    Err(e) => ModalState::Error(e),
                }
            }
        }
        MqttFocus::Url => {
            // 只在用户改动了(与「当前生效 URL」不同)时写回 config,
            // 避免把默认本地地址回写污染 cfg.broker。
            // 生效 URL = for_client().broker(config 填了用配置值,空 + 内置才默认本地)。
            let new_url = url.value().trim().to_string();
            let effective = mqtt
                .cfg
                .as_ref()
                .map(|c| c.for_client().broker)
                .unwrap_or_default();
            if new_url == effective {
                focus = MqttFocus::ClientToggle;
                mqtt_panel(tcp, ws, url, focus)
            } else if let Some(cfg) = mqtt.cfg.as_mut() {
                cfg.broker = new_url.clone();
                match crate::setup::save_mqtt(cfg, None) {
                    Ok(()) => {
                        log::info!("[mqtt] saved broker url={new_url}");
                        focus = MqttFocus::ClientToggle;
                        mqtt_panel(tcp, ws, url, focus)
                    }
                    Err(e) => ModalState::Error(format!("save url failed: {e}")),
                }
            } else {
                focus = MqttFocus::ClientToggle;
                mqtt_panel(tcp, ws, url, focus)
            }
        }
        MqttFocus::ClientToggle => {
            if let Some(h) = mqtt.client.take() {
                h.stop();
                log::info!("[mqtt] client stopped via panel");
            } else {
                let cfg = mqtt
                    .cfg
                    .as_ref()
                    .expect("mqtt_cfg_present implies mqtt_cfg")
                    .for_client();
                *mqtt.client = Some(mqtt::spawn(
                    cfg,
                    mqtt.cli_tx.clone(),
                    mqtt.tx.clone(),
                    mqtt.image_format,
                ));
                log::info!("[mqtt] client started via panel");
            }
            ModalState::None
        }
    }
}

/// MqttPanel 上编辑聚焦的输入字段(BrokerStart/ClientToggle 聚焦时不编辑)。
/// 端口只收数字;URL 收 URL 合法字符。方向/Home/End/Backspace 通用。
fn mqtt_panel_edit(
    focus: MqttFocus,
    bytes: &[u8],
    tcp: &mut tui_input::Input,
    ws: &mut tui_input::Input,
    url: &mut tui_input::Input,
) {
    let req = match focus {
        MqttFocus::Tcp => parse_input_request(
            bytes,
            |c| c.is_ascii_digit(),
            tcp.value().chars().count(),
            5,
        ),
        MqttFocus::Ws => {
            parse_input_request(bytes, |c| c.is_ascii_digit(), ws.value().chars().count(), 5)
        }
        MqttFocus::Url => {
            parse_input_request(bytes, url_char_allowed, url.value().chars().count(), 60)
        }
        // BrokerStart / ClientToggle 聚焦时不编辑输入字段。
        MqttFocus::BrokerStart | MqttFocus::ClientToggle => return,
    };
    if let Some(req) = req {
        match focus {
            MqttFocus::Tcp => {
                tcp.handle(req);
            }
            MqttFocus::Ws => {
                ws.handle(req);
            }
            MqttFocus::Url => {
                url.handle(req);
            }
            // 上面已对 BrokerStart/ClientToggle 提前 return,这里不可达。
            MqttFocus::BrokerStart | MqttFocus::ClientToggle => {}
        }
    }
}

/// 处理 MqttPanel 上的按键:Tab/↑↓ 切聚焦项、Esc 关闭、Enter 按聚焦项动作,
/// 否则编辑聚焦的输入字段。返回新的面板状态。
async fn handle_mqtt_panel(
    mut tcp: tui_input::Input,
    mut ws: tui_input::Input,
    mut url: tui_input::Input,
    mut focus: MqttFocus,
    bytes: &[u8],
    mqtt: &mut MqttCtx<'_>,
) -> ModalState {
    // 导航:Tab / ↓ 下一项,↑ 上一项。
    if bytes == b"\t" || bytes == b"\x1b[B" {
        focus = focus.next();
    } else if bytes == b"\x1b[A" {
        focus = focus.prev();
    } else if bytes == b"\x1b" {
        return ModalState::None; // Esc 取消
    } else if bytes == b"\r" || bytes == b"\n" {
        return mqtt_panel_enter(tcp, ws, url, focus, mqtt).await;
    } else {
        mqtt_panel_edit(focus, bytes, &mut tcp, &mut ws, &mut url);
    }
    mqtt_panel(tcp, ws, url, focus)
}

/// 处理模态打开时的按键:按对话框类型路由。返回新的 modal 状态。
async fn handle_modal_input(
    modal: ModalState,
    bytes: &[u8],
    http: &mut HttpBtnState,
    mqtt: &mut MqttCtx<'_>,
    screenshot_tx: &ScreenshotTx,
    image_format: crate::protocol::ImageFormat,
) -> ModalState {
    match modal {
        ModalState::PortInput { input } => {
            handle_port_input(input, bytes, http, screenshot_tx, image_format).await
        }
        ModalState::MqttPanel {
            tcp,
            ws,
            url,
            focus,
        } => handle_mqtt_panel(tcp, ws, url, focus, bytes, mqtt).await,
        ModalState::Error(_) => ModalState::None, // 任意键关闭
        ModalState::None => ModalState::None,
    }
}

/// footer 点击命中检测:Quit(右)/ HTTP(off 态,左)/ MQTT(配了 [mqtt] 才显示)。
/// 命中按钮分别返回退出、打开对应对话框;无命中返回 None。
fn handle_click(
    col: u16,
    row: u16,
    term_size: (u16, u16),
    http: &HttpBtnState,
    default_bind: &str,
    mqtt_cfg: Option<&MqttConfig>,
    mqtt: MqttButtonsState,
) -> ClickOutcome {
    let area = Rect::new(0, 0, term_size.0, term_size.1);
    // 退出按钮(右):发 0x03 + 退出循环,等同 Ctrl+C。
    let quit = quit_button_rect(area, "Quit");
    if hit_test(col, row, quit) {
        return ClickOutcome::Quit;
    }
    // HTTP 按钮(左,off 态):打开地址输入框。
    if matches!(http, HttpBtnState::Off) {
        let btn = http_button_rect(area, "HTTP off");
        if hit_test(col, row, btn) {
            return ClickOutcome::Modal(Box::new(ModalState::PortInput {
                input: tui_input::Input::new(default_bind.to_string()),
            }));
        }
    }
    // MQTT 按钮(HTTP 右边):只要配了 [mqtt] 就开控制面板(起停都在面板里,不看 off 态)。
    if let Some(cfg) = mqtt_cfg {
        let hl = button_label("HTTP", http);
        let mlabel = mqtt_button_label(&mqtt);
        let mbtn = mqtt_button_rect(area, &hl, &mlabel);
        if hit_test(col, row, mbtn) {
            return ClickOutcome::Modal(Box::new(ModalState::MqttPanel {
                tcp: tui_input::Input::new(cfg.builtin_port.to_string()),
                ws: tui_input::Input::new(cfg.builtin_ws_port.to_string()),
                // 预填生效 URL(见 MqttConfig::for_client):填了 broker 就用配置值,
                // 没填 + 内置 broker 才默认本地。
                url: tui_input::Input::new(cfg.for_client().broker),
                focus: MqttFocus::Tcp,
            }));
        }
    }
    ClickOutcome::None
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    command: Vec<String>,
    mut rx: ClientRx,
    ui_rx: &mut mpsc::Receiver<crate::ui::UIEvent>,
    tx: ServerTx,
    cli_tx: mpsc::Sender<ClientMessage>,
    screenshot_tx: ScreenshotTx,
    default_bind: String,
    mut mqtt_cfg: Option<MqttConfig>,
    mut screenshot_rx: mpsc::Receiver<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    tui: &mut crate::ui::TuiTerminal,
    ui_title: &mut String,
    image_format: crate::protocol::ImageFormat,
    auto_submit: bool,
) -> anyhow::Result<()> {
    // headless(无 TTY,例如纯 MQTT、无 WS 客户端)时 crossterm 返回 0×0 而非 Err,
    // unwrap_or 兜不住;尺寸为 0 会让 vt80 渲染时 overflow panic。拿不到有效尺寸时默认 80×24。
    let (cols, rows) = match crossterm::terminal::size() {
        Ok((c, r)) if c > 0 && r > 0 => (c, r),
        _ => (80, 24),
    };
    let vt_cols = cols.saturating_sub(TUI_COLS_PADDING);
    let vt_rows = rows.saturating_sub(TUI_ROWS_PADDING);

    let process_command = command.first().unwrap().as_str();

    let mut agent_type = crate::terminal::agent::AgentType::new(process_command);

    let mut terminal = crate::terminal::pty::new_with_command(
        process_command,
        &command[1..],
        &[(
            "VIBETTY_PORT".to_string(),
            default_bind
                .rsplit_once(':')
                .map(|(_, p)| p)
                .unwrap_or("0")
                .to_string(),
        )],
        (vt_rows, vt_cols),
    )
    .await?;

    let mut vt_parser =
        vt100::Parser::new_with_callbacks(vt_rows, vt_cols, 8096, WindowCallbacks::new());

    let mut last_frame_time = std::time::Instant::now() - IMAGE_FRAME_INTERVAL;

    // 「一页」的行数:由最近一次 sync 的像素高度推算(能塞下的行数 − 1)。滚动 rows=0 时用它。
    // 还没 sync 过就用初始终端行数兜底。
    let mut page_rows: u16 = vt_rows;

    // footer HTTP 按钮状态 + 对话框状态;启动时都 off。
    let mut http = HttpBtnState::Off;
    let mut modal = ModalState::None;
    // 终端整体尺寸(单元格),用于点击命中检测;随 Resize 更新。
    let mut term_size: (u16, u16) = (cols, rows);

    // MQTT:`enable` / `builtin_broker` 当 auto-start 标志。broker 只起不停(rumqttd 无 shutdown);
    // client 可停/重起(oneshot cancel,见 mqtt::MqttHandle)。
    let (mut mqtt_broker_on, mut mqtt_client) =
        autostart_mqtt(&mqtt_cfg, &cli_tx, &tx, image_format);
    // footer MQTT 按钮:只要配了 [mqtt] 就显示(不再绑 builtin_broker)。
    let mqtt_cfg_present = mqtt_cfg.is_some();

    // footer 按钮悬停态。HoveredBtn 是 Copy,直接当参数传进 redraw,比捕获个 Cell 直白。
    let mut hover = HoveredBtn::None;

    // 统一重绘:画 screen/title + 按钮状态 + 对话框 + 悬停高亮。`tui` 仅由此闭包借用。
    let mut redraw = |screen: &Arc<vt100::Screen>,
                      title: &str,
                      http: &HttpBtnState,
                      mqtt_broker_on: bool,
                      mqtt_client_on: bool,
                      modal: &ModalState,
                      hover: HoveredBtn| {
        let mqtt = MqttButtonsState {
            broker_on: mqtt_broker_on,
            client_on: mqtt_client_on,
        };
        let mqtt_opt = mqtt_cfg_present.then_some(&mqtt);
        let _ =
            tui.draw(|f| crate::ui::render_frame(f, screen, title, http, mqtt_opt, modal, hover));
    };

    // presence 心跳由 ws 主循环驱动:每 PRESENCE_INTERVAL_SECS 发一次 Presence(含 title+state)。
    // interval 首次 tick 立即返回 → 充当上线公告(初始 presence)。
    let mut presence_interval =
        tokio::time::interval(std::time::Duration::from_secs(mqtt::PRESENCE_INTERVAL_SECS));

    loop {
        let event = tokio::select! {
            result = terminal.read_pty_output() => match result {
                Ok(r) => TerminalEvent::PtyOutput(r),
                Err(e) => {
                    log::error!("[{}] Error reading PTY output: {:?}", terminal.session_id(), e);
                    TerminalEvent::Error
                }
            },
            msg = rx.recv() => match msg {
                Some(input) => TerminalEvent::Input(input),
                None => TerminalEvent::InputClosed,
            },
            ui_evt = ui_rx.recv() => match ui_evt {
                Some(evt) => TerminalEvent::UIEvent(evt),
                None => {
                    log::error!("[{}] UI event channel closed", terminal.session_id());
                    TerminalEvent::Error
                }
            },
            req = screenshot_rx.recv() => match req {
                Some(resp_tx) => TerminalEvent::ScreenGetter(resp_tx),
                None => {
                    log::error!("[{}] Screenshot request channel closed", terminal.session_id());
                    TerminalEvent::Error
                }
            },
            _ = presence_interval.tick() => {
                // presence 心跳(上线/状态):定期由 ws 触发,含当前 title + agent 状态。
                let _ = tx.send(ServerMessage::Presence {
                    title: ui_title.clone(),
                    state: agent_type.state(),
                });
                continue;
            },
        };

        match event {
            TerminalEvent::ScreenGetter(getter) => {
                let screen = vt_parser.screen().clone();
                let result = render_screen_to_image(&screen, image_format, None);

                let jpeg = match result {
                    Ok(data) => Ok(data),
                    Err(e) => Err(e.to_string()),
                };
                let _ = getter.send(jpeg);
            }
            TerminalEvent::PtyOutput(output) => {
                // 空 PTY 输出 = EOF / 子进程已退出(reader 线程读到 Ok(0) 后停掉、
                // read_rx 关闭)。此时优雅退出循环,由 main.rs 走 TUI cleanup;否则会空读 busy-loop。
                if output.is_empty() {
                    log::info!(
                        "[{}] PTY closed (child exited), shutting down",
                        terminal.session_id()
                    );
                    break;
                }
                log::trace!(
                    "[{}] PTY : hide_cursor {}",
                    terminal.session_id(),
                    vt_parser.screen().hide_cursor()
                );
                log::trace!("[{}] PTY output: {output:?}", terminal.session_id());
                vt_parser.process(output.as_bytes());

                // Check for title update from callbacks
                let mut became_waiting = false;
                {
                    let cb = vt_parser.callbacks_mut();
                    if cb.update_title {
                        cb.update_title = false;
                        let new_title = cb.title.clone();
                        let state_changed = agent_type.update_by_title(&new_title);
                        *ui_title = new_title;
                        log::info!(
                            "[{}] Window title updated: {}",
                            terminal.session_id(),
                            ui_title
                        );
                        // 状态翻转(codex/claude working↔waiting)→ 立即重发 presence(含新 title+state)。
                        if state_changed {
                            let _ = tx.send(ServerMessage::Presence {
                                title: ui_title.clone(),
                                state: agent_type.state(),
                            });
                            // 翻转到 waiting:agent 在等用户操作,把滚动拉回最新(scrollback=0),
                            // 免得用户停在历史区看漏新提示。cb 正借用 vt_parser,先标记、出块再改 screen。
                            became_waiting = agent_type.state().is_waiting();
                        }
                    }
                }
                // 出块后 screen_mut 可用:重置滚动到最新(0=底部),随后的 redraw/发图都基于此。
                if became_waiting {
                    vt_parser.screen_mut().set_scrollback(0);
                }

                // Render directly to TUI
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );
                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }

                // Generate JPEG and broadcast chunks for img subscribers (rate limited)
                let now = std::time::Instant::now();
                if (now.duration_since(last_frame_time) >= IMAGE_FRAME_INTERVAL
                    && screen.scrollback() == 0)
                    || agent_type.state().is_waiting()
                {
                    last_frame_time = now;
                    send_screen(&tx, screen);
                }
            }

            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(bytes)) => {
                if matches!(modal, ModalState::None) {
                    log::info!("UI Input: {:?}", String::from_utf8_lossy(&bytes));
                    terminal.send_bytes(&bytes).await?;
                } else {
                    // 模态打开:按键路由给对话框,不进 PTY。返回新 modal 后重绘。
                    modal = {
                        let mut mqtt = MqttCtx {
                            cfg: &mut mqtt_cfg,
                            broker_on: &mut mqtt_broker_on,
                            client: &mut mqtt_client,
                            cli_tx: &cli_tx,
                            tx: &tx,
                            image_format,
                        };
                        handle_modal_input(
                            std::mem::take(&mut modal),
                            &bytes,
                            &mut http,
                            &mut mqtt,
                            &screenshot_tx,
                            image_format,
                        )
                        .await
                    };
                    let screen = Arc::new(vt_parser.screen().clone());
                    redraw(
                        &screen,
                        ui_title,
                        &http,
                        mqtt_broker_on,
                        mqtt_client.is_some(),
                        &modal,
                        hover,
                    );
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Click { col, row }) => {
                // 模态打开时忽略点击。
                if matches!(modal, ModalState::None) {
                    let mqtt_state = MqttButtonsState {
                        broker_on: mqtt_broker_on,
                        client_on: mqtt_client.is_some(),
                    };
                    match handle_click(
                        col,
                        row,
                        term_size,
                        &http,
                        &default_bind,
                        mqtt_cfg.as_ref(),
                        mqtt_state,
                    ) {
                        ClickOutcome::Quit => {
                            log::info!("Quit button clicked, sending Ctrl+C (0x03)");
                            terminal.send_bytes(&[0x03]).await?;
                            break;
                        }
                        ClickOutcome::Modal(m) => {
                            // 打开模态时清悬停态:模态期间 Hover 分支被挡不更新,
                            // 清掉可避免关闭模态后残留上次的高亮。
                            hover = HoveredBtn::None;
                            modal = *m;
                            let screen = Arc::new(vt_parser.screen().clone());
                            redraw(
                                &screen,
                                ui_title,
                                &http,
                                mqtt_broker_on,
                                mqtt_client.is_some(),
                                &modal,
                                hover,
                            );
                        }
                        ClickOutcome::None => {}
                    }
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Hover { col, row }) => {
                // 模态打开时忽略悬停(footer 被遮)。且只在悬停按钮「变化」时才重绘——
                // ?1003h 会对光标划过的每个单元格都报一次 Moved,在终端区/同一按钮内移动时
                // 悬停态不变,这里直接跳过,避免刷屏。
                if matches!(modal, ModalState::None) {
                    let mqtt_state = MqttButtonsState {
                        broker_on: mqtt_broker_on,
                        client_on: mqtt_client.is_some(),
                    };
                    let now = button_row_at(
                        col,
                        row,
                        Rect::new(0, 0, term_size.0, term_size.1),
                        &http,
                        mqtt_cfg_present.then_some(&mqtt_state),
                    );
                    if now != hover {
                        hover = now;
                        let screen = Arc::new(vt_parser.screen().clone());
                        redraw(
                            &screen,
                            ui_title,
                            &http,
                            mqtt_broker_on,
                            mqtt_client.is_some(),
                            &modal,
                            hover,
                        );
                    }
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollUp { rows })
            | TerminalEvent::Input(ClientMessage::ScrollUp { rows }) => {
                let delta = scroll_delta(rows, page_rows);
                let before = vt_parser.screen().scrollback();
                vt_parser
                    .screen_mut()
                    .set_scrollback(before.saturating_add(delta));
                let after = vt_parser.screen().scrollback();
                log::info!("ScrollUp rows={rows} delta={delta} offset {before} -> {after}");
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );

                // scroll 已到边界、scrollback 没变 → 屏幕内容无变化,不重发图。
                if after != before {
                    send_screen(&tx, screen);
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollDown { rows })
            | TerminalEvent::Input(ClientMessage::ScrollDown { rows }) => {
                let delta = scroll_delta(rows, page_rows);
                let before = vt_parser.screen().scrollback();
                vt_parser
                    .screen_mut()
                    .set_scrollback(before.saturating_sub(delta));
                let after = vt_parser.screen().scrollback();
                log::info!("ScrollDown rows={rows} delta={delta} offset {before} -> {after}");
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );

                // scroll 已到边界、scrollback 没变 → 屏幕内容无变化,不重发图。
                if after != before {
                    send_screen(&tx, screen);
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Resize(cols, rows)) => {
                log::info!("Resize: cols={}, rows={}", cols, rows);
                term_size = (cols, rows);
                let vt_cols = cols.saturating_sub(TUI_COLS_PADDING);
                let vt_rows = rows.saturating_sub(TUI_ROWS_PADDING);
                vt_parser.screen_mut().set_size(vt_rows, vt_cols);
                let _ = terminal.resize(vt_rows, vt_cols);
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );
                send_screen(&tx, screen);
            }
            TerminalEvent::Input(ClientMessage::Sync { width, height }) => {
                // sync 按客户端像素 (width,height) 换算 cols×rows:各减去两侧 padding 后除以
                // 字符格尺寸。整张图(= cols×rows 网格 + 四周 padding)就与 sync 的 (width,height)
                // 对齐——之前只换算 cols、rows 保留旧值,图片高度和 sync 对不上。(字符网格 8×18
                // 精度内,会有 <1 格的 floor 余量。)height 另算 page_rows(= 能塞下 − 1)供 scroll。
                page_rows = page_rows_from_height(height);
                let avail_w = (width as u32).saturating_sub(2 * SCREEN_PADDING);
                let avail_h = (height as u32).saturating_sub(2 * SCREEN_PADDING);
                let cols = (avail_w / SCREEN_CHAR_WIDTH).max(8) as u16; // 最低 8 列
                let rows = (avail_h / SCREEN_CHAR_HEIGHT).max(2) as u16; // 最低 2 行(防 vt100 0 行)
                // 尺寸没变就不 resize(避免抖屏);但 sync 本身也是「请求刷屏」,仍回送屏幕。
                let (cur_rows, cur_cols) = vt_parser.screen().size();
                if cur_cols != cols || cur_rows != rows {
                    log::info!(
                        "Sync: {width}×{height}px -> resize PTY {cur_cols}×{cur_rows} -> {cols}×{rows}"
                    );
                    vt_parser.screen_mut().set_size(rows, cols);
                    let _ = terminal.resize(rows, cols);
                } else {
                    log::debug!("Sync: size unchanged ({cols}×{rows}), skip resize");
                }
                let screen = Arc::new(vt_parser.screen().clone());
                // 立即重绘本地 TUI,否则要等下一次 PTY 输出才看得到 resize/居中效果。
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );
                send_screen(&tx, screen);
            }
            TerminalEvent::Input(ClientMessage::PtyInput(input)) => {
                log::info!(
                    "Sending input to terminal: {:?}",
                    String::from_utf8_lossy(&input)
                );

                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::Input(ClientMessage::Input(text)) => {
                log::info!("Sending text input to terminal: {:?}", text);
                terminal.send_text(&text).await?;
                if auto_submit {
                    terminal.send_enter().await?;
                    vt_parser.screen_mut().set_scrollback(3);
                } else {
                    vt_parser.screen_mut().set_scrollback(0);
                }
            }
            TerminalEvent::InputClosed | TerminalEvent::Error => {
                log::error!("Input channel closed or error occurred, terminating terminal loop");
                break;
            }
        }
    }

    Ok(())
}

/// Render a vt100 screen to image bytes (JPEG or PNG)
pub(crate) fn render_screen_to_image(
    screen: &vt100::Screen,
    format: crate::protocol::ImageFormat,
    target_size: Option<(u16, u16)>, // 把图片精确补/裁到该 (width,height),不足补背景色(主要用于 MQTT 出站)
) -> anyhow::Result<Vec<u8>> {
    let config = crate::screenshot::ScreenshotConfig {
        show_decorations: false,
        ..Default::default()
    };

    let image = crate::screenshot::capture_screen(screen, &config)
        .map_err(|e| anyhow::anyhow!("Failed to capture screen: {}", e))?;

    let mut dyn_image = image::DynamicImage::ImageRgba8(image);

    // 精确补齐到目标尺寸(主要用于 MQTT 出站):画布做到 (tw,th) 并填满背景色,再把当前图
    // overlay 到 (0,0)——原图比目标小则四周补背景色,比目标大则等价裁切。PTY 已按 sync
    // resize 成 cols×rows,故原图通常 ≤ 目标,只差字符网格 8×18 的 floor 余量,走「补背景色」。
    if let Some((tw, th)) = target_size {
        let (tw, th) = (tw as u32, th as u32);
        if dyn_image.width() != tw || dyn_image.height() != th {
            let mut canvas = image::RgbaImage::from_pixel(tw, th, image::Rgba([30, 30, 30, 255]));
            image::imageops::overlay(&mut canvas, &dyn_image.to_rgba8(), 0, 0);
            dyn_image = image::DynamicImage::ImageRgba8(canvas);
        }
    }

    // Encode based on format
    let mut buf = std::io::Cursor::new(Vec::new());
    match format {
        crate::protocol::ImageFormat::Png => {
            let bytes = crate::png_encode::encode_paletted_png(&dyn_image)
                .map_err(|e| anyhow::anyhow!("Failed to encode PNG: {}", e))?;
            buf.get_mut().extend_from_slice(&bytes);
        }
        crate::protocol::ImageFormat::Jpeg => {
            // Convert to RGB for JPEG
            let rgb_image = dyn_image.to_rgb8();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
            encoder
                .encode(
                    rgb_image.as_raw(),
                    rgb_image.width(),
                    rgb_image.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| anyhow::anyhow!("Failed to encode JPEG: {}", e))?;
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported image format: {:?}", format));
        }
    }

    Ok(buf.into_inner())
}

/// HTTP handler for GET /screenshot — returns the current terminal screen
/// rendered as an image whose format/MIME is determined by `image_format`.
pub async fn screenshot_handler(State(state): State<AppState>) -> impl IntoResponse {
    let image_format = state.image_format;
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

    if state.screenshot_tx.send(resp_tx).await.is_err() {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            "Screenshot service unavailable".to_string(),
        )
            .into_response();
    }

    match resp_rx.await {
        Ok(Ok(image_data)) => {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                image_format.mime_type().parse().unwrap(),
            );
            headers.insert(
                axum::http::header::CACHE_CONTROL,
                "no-cache".parse().unwrap(),
            );
            (axum::http::StatusCode::OK, headers, image_data).into_response()
        }
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            format!("Failed to render screenshot: {}", e),
        )
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            "Screenshot request timed out".to_string(),
        )
            .into_response(),
    }
}
