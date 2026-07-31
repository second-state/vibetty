use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    fit_button_rect, hit_test, http_button_rect, mqtt_button_label, mqtt_button_rect,
    quit_button_rect,
};

/// 终端截图里单个字符单元格的像素宽。`vibetty-screenshot` 在 font_size=14.0 +
/// 内嵌字体 Sarasa Mono SC Light(swash 后端)下由 `get_char_metrics(14.0)` 实测得到。
/// 客户端 Sync 在 `pixels=true` 时发的是【像素】,要除以它换算成 PTY 列数。
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
    pub image_format: crate::protocol::OutputFormat,
}

fn send_screen(tx: &ServerTx, screen: Arc<vt100::Screen>) {
    // 广播整张屏幕;渲染成 JPEG 还是 ANSI 文本由 MQTT 端按 image_format 决定。
    let _ = tx.send(ServerMessage::Screen(screen));
}

/// 把屏幕渲染成带 ANSI 转义的「可重放」字节流(`-q text` 模式):直接用 vt300 的
/// `Screen::contents_formatted`,内联 SGR 颜色/光标定位等转义码,喂给任何终端解析器即可还原画面
/// (含颜色)。尊重 scrollback:滚到历史时输出的是历史那屏。
pub(crate) fn render_screen_to_text(screen: &vt100::Screen) -> String {
    String::from_utf8_lossy(&screen.contents_formatted()).into_owned()
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

/// 主事件循环统一的事件来源(PTY 输出 / 客户端消息 / TUI UI 事件 / 截图请求)。
enum TerminalEvent {
    Input(crate::protocol::ClientMessage),
    InputClosed,
    UIEvent(crate::ui::UIEvent),
    PtyOutput(String),
    ScreenGetter(tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>),
    Error,
}

/// 点击顶部按钮的结果:退出程序 / 打开对话框 / 重置终端尺寸 / 无命中。
/// `Modal` 装箱:另几个变体无数据,装箱避免整个枚举撑到 ModalState 的大小
/// (clippy::large_enum_variant);该值每次点击产生后立即消费,一次分配可忽略。
enum ClickOutcome {
    Quit,
    Modal(Box<ModalState>),
    FitSize,
    None,
}

/// MqttPanel 操作需要的可变状态(改端口/URL 写 config、起停 broker/client)。
/// 把这几样 `&mut` 收进一个结构体,免去面板处理函数一长串参数。
struct MqttCtx<'a> {
    cfg: &'a mut Option<MqttConfig>,
    broker_on: &'a mut bool,
    broker_alive: &'a mut Option<Arc<AtomicBool>>,
    client: &'a mut Option<mqtt::MqttHandle>,
    cli_tx: &'a mpsc::Sender<ClientMessage>,
    tx: &'a ServerTx,
    image_format: crate::protocol::OutputFormat,
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
    image_format: crate::protocol::OutputFormat,
) -> (bool, Option<mqtt::MqttHandle>, Option<Arc<AtomicBool>>) {
    let Some(cfg) = mqtt_cfg else {
        return (false, None, None);
    };
    let mut broker_on = false;
    let mut broker_alive = None;
    if cfg.builtin_broker {
        let alive = Arc::new(AtomicBool::new(true));
        match broker::spawn_builtin(cfg, alive.clone()) {
            Ok(()) => {
                broker_on = true;
                broker_alive = Some(alive);
                log::info!("[mqtt] broker auto-started on :{}", cfg.builtin_port);
            }
            Err(e) => log::warn!("[mqtt] broker auto-start failed: {e}"),
        }
    }
    let client = cfg.enable.then(|| {
        log::info!("[mqtt] client auto-started");
        mqtt::spawn(cfg.for_client(), cli_tx.clone(), tx.clone(), image_format)
    });
    (broker_on, client, broker_alive)
}

/// 处理 HTTP 端口输入框的按键:Enter 按地址起 server、Esc 取消、其余编辑输入。
async fn handle_port_input(
    mut input: tui_input::Input,
    bytes: &[u8],
    http: &mut HttpBtnState,
    screenshot_tx: &ScreenshotTx,
    image_format: crate::protocol::OutputFormat,
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
                            let alive = Arc::new(AtomicBool::new(true));
                            match broker::spawn_builtin(&cfg, alive.clone()) {
                                Ok(()) => {
                                    log::info!("[mqtt] broker started on :{t}");
                                    *mqtt.broker_on = true;
                                    *mqtt.broker_alive = Some(alive);
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
    image_format: crate::protocol::OutputFormat,
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
    // Fit 按钮(MQTT 右边或 HTTP 右边):把终端尺寸重置成当前窗口。
    let http_label = button_label("HTTP", http);
    let mqtt_label = mqtt_cfg.map(|_| mqtt_button_label(&mqtt));
    if hit_test(
        col,
        row,
        fit_button_rect(area, &http_label, mqtt_label.as_deref()),
    ) {
        return ClickOutcome::FitSize;
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
    image_format: crate::protocol::OutputFormat,
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
    let (mut mqtt_broker_on, mut mqtt_client, mut broker_alive) =
        autostart_mqtt(&mqtt_cfg, &cli_tx, &tx, image_format);
    // footer MQTT 按钮:只要配了 [mqtt] 就显示(不再绑 builtin_broker)。
    let mqtt_cfg_present = mqtt_cfg.is_some();

    // footer 按钮悬停态。HoveredBtn 是 Copy,直接当参数传进 redraw,比捕获个 Cell 直白。
    let mut hover = HoveredBtn::None;

    // 统一重绘:画 screen/title + 按钮状态 + 对话框 + 悬停高亮。`tui` 仅由此闭包借用。
    let mut redraw = |screen: &vt100::Screen,
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
        let _ = tui.draw(|f| {
            crate::ui::render_frame(f, screen, title, http, mqtt_opt, modal, hover, None)
        });
    };

    // presence 心跳由 ws 主循环驱动:每 PRESENCE_INTERVAL_SECS 发一次 Presence(含 title+state)。
    // interval 首次 tick 立即返回 → 充当上线公告(初始 presence)。
    let mut presence_interval =
        tokio::time::interval(std::time::Duration::from_secs(mqtt::PRESENCE_INTERVAL_SECS));

    // ── 启动初始化去抖 ──────────────────────────────────────────────
    // agent 刚启动时 PTY 狂输出(初始化 UI、motd、prompt 绘制…),而初始 title 往往让
    // agent 被判 Waiting(见 agent.rs),主 loop 的 is_waiting 旁路会让每次输出都广播
    // 一整张 screen JPEG → MQTT 流量爆发。这里在进主 loop 前先等 PTY 输出静默满
    // INIT_SETTLE(500ms),稳定后只发一帧 screen + 一次 presence(上线);之后再进主
    // loop,实时性不变(waiting 仍即时发、working 5s 一帧)。静默迟迟达不到时最多等
    // INIT_MAX_WAIT,避免 agent 一启动就长跑导致 ESP32 长时间黑屏。期间键盘/鼠标、
    // MQTT control/sync、HTTP screenshot 等事件都在各自 channel 里排队,进主 loop 后处理。
    const INIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    const INIT_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
    let init_started = tokio::time::Instant::now();
    let mut last_pty_at = tokio::time::Instant::now();
    'init: loop {
        let settle_deadline = last_pty_at + INIT_SETTLE;
        let max_deadline = init_started + INIT_MAX_WAIT;
        tokio::select! {
            result = terminal.read_pty_output() => match result {
                Ok(output) if !output.is_empty() => {
                    vt_parser.process(output.as_bytes());
                    // title / agent 状态更新,与主 loop PtyOutput 分支一致。
                    {
                        let cb = vt_parser.callbacks_mut();
                        if cb.update_title {
                            cb.update_title = false;
                            let new_title = cb.title.clone();
                            agent_type.update_by_title(&new_title);
                            *ui_title = new_title;
                        }
                    }
                    last_pty_at = tokio::time::Instant::now();
                    // 本地 TUI redraw(让本地用户看到启动过程);**不**广播 screen/presence,
                    // MQTT 在此期间保持安静。
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
                Ok(_) => break 'init, // 空 = PTY EOF(子进程秒退),交给主 loop 处理退出。
                Err(e) => {
                    log::error!(
                        "[{}] init PTY read error: {:?}",
                        terminal.session_id(),
                        e
                    );
                    break 'init;
                }
            },
            _ = tokio::time::sleep_until(settle_deadline) => break 'init, // 静默满 500ms → settled
            _ = tokio::time::sleep_until(max_deadline) => break 'init,     // 上限保护
        }
    }
    // 初始化完成(或达上限):发第一帧 screen + 上线 presence,然后进入主 loop。
    let screen = Arc::new(vt_parser.screen().clone());
    send_screen(&tx, screen);
    let _ = tx.send(ServerMessage::Presence {
        title: ui_title.clone(),
        state: agent_type.state(),
    });
    log::info!(
        "[{}] init settled after {}ms, sent first screen + presence",
        terminal.session_id(),
        init_started.elapsed().as_millis()
    );

    // screen 发送去抖(无条件尾部去抖):每次 PTY 输出都激活 SCREEN_DEBOUNCE(100ms)计时器,
    // 期间有新输出就刷新(重设)计时器,停顿满 100ms 才发最新帧。把一次 burst 合并成一张图,
    // 省流量。ESP32 保留最后一帧不会黑屏,持续输出期间静默(等停顿)即可。
    const SCREEN_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(100);
    let mut pending_screen: Option<Arc<vt100::Screen>> = None;
    let mut send_deadline: Option<tokio::time::Instant> = None;
    // close 开关(Sync.close):true 时暂停 PTY 输出触发的自主 screen 推送。客户端不看时关掉省流量。
    let mut screen_closed = false;

    // resize 后 PTY 会刷一大段重绘 burst(codex/helix 这类 TUI 收到 SIGWINCH 全量重绘)。
    // 这段时间不转发 pty_out 增量 / 不发屏帧(吸收掉,vt300 已把输出吃进 screen 状态),
    // 等停顿满 RESIZE_SETTLE(500ms)再发一帧全屏,免得把重绘 burst 当增量灌给 ESP32。
    const RESIZE_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    let mut resize_settle_until: Option<tokio::time::Instant> = None;

    loop {
        // 内置 broker 可能在后台悄悄退出(端口被占 bind 失败、panic 等)。broker 线程退出时
        // 会把 alive AtomicBool 置 false,这里每轮检测:一旦发现挂了就把按钮状态改回 off + 重绘,
        // 不再假装 broker 在跑。(idle 时最长等下一次事件/presence tick 才察觉。)
        if let Some(a) = broker_alive.as_ref()
            && mqtt_broker_on
            && !a.load(Ordering::Relaxed)
        {
            mqtt_broker_on = false;
            log::warn!("[mqtt] builtin broker exited; marking broker off");
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
        // 每轮 copy 最新 send_deadline(Option<Instant> 是 Copy),让下面去抖 async 分支按最新
        // deadline 等待;None 时该分支永不就绪。
        let deadline_copy = send_deadline;
        let settle_copy = resize_settle_until;
        // biased:按声明顺序优先。把入站控制(MQTT sync/pty_in/close)和本地按键排在 PTY 输出前面——
        // 狂输出时每条 PTY 都触发 redraw(重),随机 select! 会让 sync/按键被反复推迟;这样保证
        // 控制消息在下一段 PTY 之前先处理掉。PTY 是流量大头,排在控制之后不会饿死(控制是稀疏的)。
        let event = tokio::select! {
            biased;
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
            result = terminal.read_pty_output() => match result {
                Ok(r) => TerminalEvent::PtyOutput(r),
                Err(e) => {
                    log::error!("[{}] Error reading PTY output: {:?}", terminal.session_id(), e);
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
            // screen 去抖到期:发送最新 pending screen,清空去抖状态。
            _ = async {
                match deadline_copy {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Some(screen) = pending_screen.take() {
                    send_screen(&tx, screen);
                }
                send_deadline = None;
                continue;
            },
            // resize settle 到期:PTY 重绘 burst 已静默满 500ms,发一帧全屏,退出 settle。
            _ = async {
                match settle_copy {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                resize_settle_until = None;
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
                continue;
            },
        };

        match event {
            TerminalEvent::ScreenGetter(getter) => {
                let screen = vt_parser.screen().clone();
                // text 模式返回纯文本字节,否则返回 JPEG(/screenshot 的 MIME 由 mime_type() 定)。
                let result: Result<Vec<u8>, String> = if image_format.is_text() {
                    Ok(render_screen_to_text(&screen).into_bytes())
                } else {
                    render_screen_to_image(&screen, image_format, None).map_err(|e| e.to_string())
                };
                let _ = getter.send(result);
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
                log::debug!(
                    "[{}] PTY output len: {} bytes (screen={} sb={})",
                    terminal.session_id(),
                    output.len(),
                    if vt_parser.screen().alternate_screen() {
                        "alt"
                    } else {
                        "main"
                    },
                    vt_parser.screen().scrollback(),
                );
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
                        log::debug!(
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

                // 本地 redraw:直接借用 vt_parser 的 screen,不 clone(text 模式广播的是原始字节、
                // 根本不碰 screen;只有 jpeg 入队 pending 时才需要拥有,见下)。
                redraw(
                    vt_parser.screen(),
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                    hover,
                );
                if resize_settle_until.is_some() {
                    // resize 后的重绘 burst:吸收,不转发 delta / 不入队去抖;vt300 已把输出吃进
                    // screen 状态,等 settle 到期发全屏即可。每来一段就重设 500ms 计时器。
                    resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                } else {
                    // text 模式:实时把原始 PTY 字节发给 ESP32(它自己跑终端模拟器渲染),仅在未 close
                    // 时发。⚠️ 高频 PtyOutput 每条 broadcast,可能撑爆 broadcast 容量(1024)触发 Lagged;
                    // 真扛不住再改独立通道。
                    if image_format.is_text() && !screen_closed {
                        let _ = tx.send(ServerMessage::PtyOutput(output.clone().into_bytes()));
                    }

                    // jpeg 模式:走 screen 去抖(每次输出入队 pending + 重设 100ms 计时器,停顿满发
                    // 最新帧)。text 模式不发屏幕帧(靠上面的 pty_out 实时流)。看历史(scrollback!=0)
                    // 且非 waiting、或 close=true 暂停时,都不入队。**只有 jpeg 才 clone 整屏**(text 不需要)。
                    if !image_format.is_text()
                        && !screen_closed
                        && (vt_parser.screen().scrollback() == 0 || agent_type.state().is_waiting())
                    {
                        pending_screen = Some(Arc::new(vt_parser.screen().clone()));
                        send_deadline = Some(tokio::time::Instant::now() + SCREEN_DEBOUNCE);
                    }
                }
            }

            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(bytes)) => {
                if matches!(modal, ModalState::None) {
                    log::debug!("UI Input: {:?}", String::from_utf8_lossy(&bytes));
                    terminal.send_bytes(&bytes).await?;
                } else {
                    // 模态打开:按键路由给对话框,不进 PTY。返回新 modal 后重绘。
                    modal = {
                        let mut mqtt = MqttCtx {
                            cfg: &mut mqtt_cfg,
                            broker_on: &mut mqtt_broker_on,
                            broker_alive: &mut broker_alive,
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
                        ClickOutcome::FitSize => {
                            // 把终端尺寸重置成当前 TUI 窗口(适配本地;撤销 ESP32 sync 改的小屏)。
                            let vt_cols = term_size.0.saturating_sub(TUI_COLS_PADDING);
                            let vt_rows = term_size.1.saturating_sub(TUI_ROWS_PADDING);
                            vt_parser.screen_mut().set_size(vt_rows, vt_cols);
                            let _ = terminal.resize(vt_rows, vt_cols);
                            log::info!("Fit: resize terminal to {vt_cols}x{vt_rows}");
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
                            // resize 触发 PTY 重绘 burst:进入 settle 吸收,等 500ms 静默后发全屏。
                            resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                            pending_screen = None;
                            send_deadline = None;
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
                log::debug!("ScrollUp rows={rows} delta={delta} offset {before} -> {after}");
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
                log::debug!("ScrollDown rows={rows} delta={delta} offset {before} -> {after}");
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
                log::debug!("Resize: cols={}, rows={}", cols, rows);
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
                // resize 触发 PTY 重绘 burst:进入 settle 吸收,等 500ms 静默后发全屏。
                resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                pending_screen = None;
                send_deadline = None;
            }
            TerminalEvent::Input(ClientMessage::Sync {
                width,
                height,
                pixels,
                close,
            }) => {
                // close 开关:控制服务端【自主】推送(PTY 输出触发的 screen/screen_text)。
                // close=true → 暂停并清掉在途的去抖帧;close=false → 恢复。sync 响应仍照发。
                screen_closed = close;
                if screen_closed {
                    // 在途的去抖帧由本 arm 末尾的 settle 入口统一清空(pending_screen/send_deadline)。
                    log::debug!(
                        "[{}] sync close=true: autonomous screen push paused",
                        terminal.session_id()
                    );
                }
                // pixels=true:width/height 是像素,各减去两侧 padding 后除以字符格尺寸换算
                // cols×rows(整张图 = cols×rows 网格 + 四周 padding,与 sync 像素尺寸对齐;8×18
                // 精度内有 <1 格 floor 余量)。pixels=false:width/height 已是字符列/行,直接用
                // (仍兜底 8×2,防 vt100 0 行)。两种模式都以最终 rows 算 page_rows(留两行重叠)。
                let (cols, rows) = if pixels {
                    let avail_w = (width as u32).saturating_sub(2 * SCREEN_PADDING);
                    let avail_h = (height as u32).saturating_sub(2 * SCREEN_PADDING);
                    (
                        (avail_w / SCREEN_CHAR_WIDTH).max(8) as u16,
                        (avail_h / SCREEN_CHAR_HEIGHT).max(2) as u16,
                    )
                } else {
                    (width.max(8), height.max(2))
                };
                page_rows = rows.saturating_sub(2).max(1);
                // 尺寸没变就不 resize(避免抖屏)。
                let (cur_rows, cur_cols) = vt_parser.screen().size();
                let resized = cur_cols != cols || cur_rows != rows;
                if resized {
                    log::debug!(
                        "Sync: {width}×{height}{} -> resize PTY {cur_cols}×{cur_rows} -> {cols}×{rows}",
                        if pixels { "px" } else { "cells" }
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
                if close {
                    // close=true:客户端暂停(不看),不发屏;清掉在途去抖/settle。
                    resize_settle_until = None;
                    pending_screen = None;
                    send_deadline = None;
                } else {
                    // close=false:立刻回送一帧(sync 响应),客户端在看、要当前画面。
                    send_screen(&tx, screen);
                    // 发生了 resize → PTY 会重绘 burst:进入 settle 吸收,500ms 静默后再发一帧最终全屏。
                    if resized {
                        resize_settle_until = Some(tokio::time::Instant::now() + RESIZE_SETTLE);
                        pending_screen = None;
                        send_deadline = None;
                    }
                }
            }
            TerminalEvent::Input(ClientMessage::PtyInput(input)) => {
                log::debug!(
                    "Sending input to terminal: {:?}",
                    String::from_utf8_lossy(&input)
                );

                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::Input(ClientMessage::Input(text)) => {
                log::debug!("Sending text input to terminal: {:?}", text);
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

/// Render a vt100 screen to JPEG bytes(质量档位由 `format` 决定:High/Medium 彩色,Low 黑白)
pub(crate) fn render_screen_to_image(
    screen: &vt100::Screen,
    format: crate::protocol::OutputFormat,
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

    // 按质量档位编码 JPEG:High=q85 彩色(默认),Medium=q70 彩色,Low=q50 黑白(灰度)。
    // Text 模式不该走到这里(调用方先 is_text() 分流到 render_screen_to_text)。
    let mut buf = std::io::Cursor::new(Vec::new());
    let (quality, grayscale) = match format {
        crate::protocol::OutputFormat::High => (85u8, false),
        crate::protocol::OutputFormat::Medium => (70, false),
        crate::protocol::OutputFormat::Low => (50, true),
        crate::protocol::OutputFormat::Text => {
            return Err(anyhow::anyhow!(
                "text format cannot render to image; use render_screen_to_text"
            ));
        }
    };
    {
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        if grayscale {
            let luma = dyn_image.to_luma8();
            encoder
                .encode(
                    luma.as_raw(),
                    luma.width(),
                    luma.height(),
                    image::ExtendedColorType::L8,
                )
                .map_err(|e| anyhow::anyhow!("Failed to encode JPEG: {}", e))?;
        } else {
            let rgb = dyn_image.to_rgb8();
            encoder
                .encode(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| anyhow::anyhow!("Failed to encode JPEG: {}", e))?;
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
