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
    HttpBtnState, ModalState, MqttButtonsState, MqttFocus, button_label, http_button_rect,
    mqtt_button_rect, mqtt_footer_label, quit_button_rect,
};

/// Image broadcast frame interval (ms)
const IMAGE_FRAME_INTERVAL_MS: u64 = 300;
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
/// Columns reserved for TUI decorations: the terminal pane's left + right
/// borders (1 column each).
const TUI_COLS_PADDING: u16 = 2;
/// Rows reserved for TUI decorations: header pane (3) + footer pane (3) +
/// the terminal pane's top + bottom borders (1 row each).
const TUI_ROWS_PADDING: u16 = 8;

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
    enum TerminalEvent {
        Input(crate::protocol::ClientMessage),
        InputClosed,

        UIEvent(crate::ui::UIEvent),

        PtyOutput(String),

        ScreenGetter(tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>),

        Error,
    }

    // headless(无 TTY,例如纯 MQTT、无 WS 客户端)时 crossterm 返回 0×0 而非 Err,
    // unwrap_or 兜不住;尺寸为 0 会让 vt80 渲染时 overflow panic。拿不到有效尺寸时默认 80×24。
    let (cols, rows) = match crossterm::terminal::size() {
        Ok((c, r)) if c > 0 && r > 0 => (c, r),
        _ => (80, 24),
    };
    let vt_cols = cols.saturating_sub(TUI_COLS_PADDING);
    let vt_rows = rows.saturating_sub(TUI_ROWS_PADDING);

    let mut terminal = crate::terminal::pty::new_with_command(
        command.first().unwrap().as_str(),
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

    // Frame rate limit for image broadcast (default 2 fps)
    let frame_interval = std::time::Duration::from_millis(IMAGE_FRAME_INTERVAL_MS);
    let mut last_frame_time = std::time::Instant::now() - frame_interval;

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
    let mut mqtt_broker_on = false;
    let mut mqtt_client: Option<mqtt::MqttHandle> = None;
    if let Some(cfg) = &mqtt_cfg {
        if cfg.builtin_broker {
            match broker::spawn_builtin(cfg) {
                Ok(()) => {
                    mqtt_broker_on = true;
                    log::info!("[mqtt] broker auto-started on :{}", cfg.builtin_port);
                }
                Err(e) => log::warn!("[mqtt] broker auto-start failed: {e}"),
            }
        }
        if cfg.enable {
            mqtt_client = Some(mqtt::spawn(
                cfg.for_client(),
                cli_tx.clone(),
                tx.clone(),
                image_format,
            ));
            log::info!("[mqtt] client auto-started");
        }
    }
    // footer MQTT 按钮:只要配了 [mqtt] 就显示(不再绑 builtin_broker)。
    let mqtt_cfg_present = mqtt_cfg.is_some();

    // 统一重绘:画 screen/title + 按钮状态 + 对话框。`tui` 仅由此闭包借用。
    let mut redraw = |screen: &Arc<vt100::Screen>,
                      title: &str,
                      http: &HttpBtnState,
                      mqtt_broker_on: bool,
                      mqtt_client_on: bool,
                      modal: &ModalState| {
        let mqtt = MqttButtonsState {
            broker_on: mqtt_broker_on,
            client_on: mqtt_client_on,
        };
        let mqtt_opt = mqtt_cfg_present.then_some(&mqtt);
        let _ = tui
            .draw(|f| crate::ui::render_frame(f, screen, title, "Vibetty", http, mqtt_opt, modal));
    };

    loop {
        let terminal_read_event = terminal.read_pty_output();

        let event = tokio::select! {
            result = terminal_read_event => {
                match result {
                    Ok(r) => TerminalEvent::PtyOutput(r),
                    Err(e) => {
                        log::error!("[{}] Error reading PTY output: {:?}", terminal.session_id(), e);
                        TerminalEvent::Error
                    },
                }

            },
            msg = rx.recv() => {
                match msg {
                    Some(input) => TerminalEvent::Input(input),
                    None => TerminalEvent::InputClosed,
                }
            },

            ui_evt = ui_rx.recv() => {
                match ui_evt {
                    Some(evt) => TerminalEvent::UIEvent(evt),
                    None => {
                        log::error!("[{}] UI event channel closed", terminal.session_id());
                        TerminalEvent::Error
                    }
                }
            },

            req = screenshot_rx.recv() => {
                match req {
                    Some(resp_tx) => TerminalEvent::ScreenGetter(resp_tx),
                    None => {
                        log::error!("[{}] Screenshot request channel closed", terminal.session_id());
                        TerminalEvent::Error
                    }
                }
            }
        };

        match event {
            TerminalEvent::ScreenGetter(getter) => {
                let screen = vt_parser.screen().clone();
                let mut window_scrollback = 0;
                let result = render_screen_to_image(
                    &screen,
                    None,
                    &mut window_scrollback,
                    image_format,
                    None,
                );

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
                log::trace!("[{}] PTY output: {}", terminal.session_id(), output.len());
                vt_parser.process(output.as_bytes());

                // Check for title update from callbacks
                {
                    let cb = vt_parser.callbacks_mut();
                    if cb.update_title {
                        cb.update_title = false;
                        let new_title = cb.title.clone();
                        let _ = cb;
                        *ui_title = new_title;
                    }
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
                if now.duration_since(last_frame_time) >= frame_interval
                    || ui_title.starts_with("✳")
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
                    // 模态打开:按键路由给对话框,不进 PTY。方向键/Home/End 移动光标,Esc 取消。
                    let mut next = std::mem::take(&mut modal);
                    next = match next {
                        ModalState::PortInput { mut input } => {
                            if bytes == b"\r" || bytes == b"\n" {
                                let bind = input.value().trim().to_string();
                                if bind.is_empty() {
                                    ModalState::Error("invalid address".to_string())
                                } else {
                                    let state = AppState {
                                        screenshot_tx: screenshot_tx.clone(),
                                        image_format,
                                    };
                                    match start_http(&bind, state).await {
                                        Ok(addr) => {
                                            log::info!("[http] started on {addr}");
                                            http = HttpBtnState::On(addr);
                                            ModalState::None
                                        }
                                        Err(e) => {
                                            log::warn!("[http] start failed: {e}");
                                            ModalState::Error(format!("listen failed: {e}"))
                                        }
                                    }
                                }
                            } else if bytes == b"\x1b" {
                                ModalState::None // Esc 取消
                            } else {
                                let req = if bytes == b"\x1b[D" {
                                    Some(InputRequest::GoToPrevChar)
                                } else if bytes == b"\x1b[C" {
                                    Some(InputRequest::GoToNextChar)
                                } else if bytes == b"\x1b[H" {
                                    Some(InputRequest::GoToStart)
                                } else if bytes == b"\x1b[F" {
                                    Some(InputRequest::GoToEnd)
                                } else if bytes == [0x08] || bytes == [0x7f] {
                                    Some(InputRequest::DeletePrevChar)
                                } else if bytes.len() == 1 {
                                    let c = bytes[0];
                                    if (c.is_ascii_alphanumeric()
                                        || matches!(c, b'.' | b':' | b'[' | b']'))
                                        && input.value().len() < 45
                                    {
                                        Some(InputRequest::InsertChar(c as char))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                                if let Some(req) = req {
                                    input.handle(req);
                                }
                                ModalState::PortInput { input }
                            }
                        }
                        ModalState::MqttPanel {
                            mut tcp,
                            mut ws,
                            mut url,
                            mut focus,
                        } => {
                            // 导航:Tab / ↓ 下一项,↑ 上一项(Tcp → Ws → BrokerStart → Url → ClientToggle 循环)。
                            if bytes == b"\t" || bytes == b"\x1b[B" {
                                focus = match focus {
                                    MqttFocus::Tcp => MqttFocus::Ws,
                                    MqttFocus::Ws => MqttFocus::BrokerStart,
                                    MqttFocus::BrokerStart => MqttFocus::Url,
                                    MqttFocus::Url => MqttFocus::ClientToggle,
                                    MqttFocus::ClientToggle => MqttFocus::Tcp,
                                };
                                ModalState::MqttPanel {
                                    tcp,
                                    ws,
                                    url,
                                    focus,
                                }
                            } else if bytes == b"\x1b[A" {
                                focus = match focus {
                                    MqttFocus::Tcp => MqttFocus::ClientToggle,
                                    MqttFocus::Ws => MqttFocus::Tcp,
                                    MqttFocus::BrokerStart => MqttFocus::Ws,
                                    MqttFocus::Url => MqttFocus::BrokerStart,
                                    MqttFocus::ClientToggle => MqttFocus::Url,
                                };
                                ModalState::MqttPanel {
                                    tcp,
                                    ws,
                                    url,
                                    focus,
                                }
                            } else if bytes == b"\x1b" {
                                ModalState::None // Esc 取消
                            } else if bytes == b"\r" || bytes == b"\n" {
                                // Enter 行为随聚焦项不同:
                                //  Tcp/Ws       -> 解析存盘(有改动则写 config)+ 跳到 BrokerStart。
                                //  BrokerStart  -> 起 broker(已起则无动作,rumqttd 停不掉)。
                                //  Url          -> broker URL 改动了则写回 [mqtt] broker + 跳到 ClientToggle。
                                //  ClientToggle -> 切换 client:已起则 stop(oneshot),否则 spawn。
                                match focus {
                                    MqttFocus::Tcp | MqttFocus::Ws => {
                                        match parse_and_save(&tcp, &ws, &mut mqtt_cfg) {
                                            Ok(_) => {
                                                focus = MqttFocus::BrokerStart;
                                                ModalState::MqttPanel {
                                                    tcp,
                                                    ws,
                                                    url,
                                                    focus,
                                                }
                                            }
                                            Err(e) => ModalState::Error(e),
                                        }
                                    }
                                    MqttFocus::BrokerStart => {
                                        if mqtt_broker_on {
                                            // 已起,rumqttd 停不掉 -> 无动作,留在面板。
                                            ModalState::MqttPanel {
                                                tcp,
                                                ws,
                                                url,
                                                focus,
                                            }
                                        } else {
                                            match parse_and_save(&tcp, &ws, &mut mqtt_cfg) {
                                                Ok((t, w)) => match tokio::net::TcpListener::bind((
                                                    "0.0.0.0", t,
                                                ))
                                                .await
                                                {
                                                    Ok(_) => {
                                                        let mut cfg = mqtt_cfg
                                                            .as_ref()
                                                            .expect(
                                                                "mqtt_cfg_present implies mqtt_cfg",
                                                            )
                                                            .clone();
                                                        cfg.builtin_port = t;
                                                        cfg.builtin_ws_port = w;
                                                        match broker::spawn_builtin(&cfg) {
                                                            Ok(()) => {
                                                                log::info!(
                                                                    "[mqtt] broker started on :{t}"
                                                                );
                                                                mqtt_broker_on = true;
                                                                ModalState::None
                                                            }
                                                            Err(e) => ModalState::Error(format!(
                                                                "broker start failed: {e}"
                                                            )),
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::warn!(
                                                            "[mqtt] port {t} unavailable: {e}"
                                                        );
                                                        ModalState::Error(format!(
                                                            "port {t} unavailable: {e}"
                                                        ))
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
                                        let effective = mqtt_cfg
                                            .as_ref()
                                            .map(|c| c.for_client().broker)
                                            .unwrap_or_default();
                                        if new_url == effective {
                                            focus = MqttFocus::ClientToggle;
                                            ModalState::MqttPanel {
                                                tcp,
                                                ws,
                                                url,
                                                focus,
                                            }
                                        } else if let Some(cfg) = mqtt_cfg.as_mut() {
                                            cfg.broker = new_url.clone();
                                            match crate::setup::save_mqtt(cfg, None) {
                                                Ok(()) => {
                                                    log::info!("[mqtt] saved broker url={new_url}");
                                                    focus = MqttFocus::ClientToggle;
                                                    ModalState::MqttPanel {
                                                        tcp,
                                                        ws,
                                                        url,
                                                        focus,
                                                    }
                                                }
                                                Err(e) => ModalState::Error(format!(
                                                    "save url failed: {e}"
                                                )),
                                            }
                                        } else {
                                            focus = MqttFocus::ClientToggle;
                                            ModalState::MqttPanel {
                                                tcp,
                                                ws,
                                                url,
                                                focus,
                                            }
                                        }
                                    }
                                    MqttFocus::ClientToggle => {
                                        if let Some(h) = mqtt_client.take() {
                                            h.stop();
                                            log::info!("[mqtt] client stopped via panel");
                                        } else {
                                            let cfg = mqtt_cfg
                                                .as_ref()
                                                .expect("mqtt_cfg_present implies mqtt_cfg")
                                                .for_client();
                                            mqtt_client = Some(mqtt::spawn(
                                                cfg,
                                                cli_tx.clone(),
                                                tx.clone(),
                                                image_format,
                                            ));
                                            log::info!("[mqtt] client started via panel");
                                        }
                                        ModalState::None
                                    }
                                }
                            } else {
                                // 编辑聚焦的输入字段(BrokerStart/ClientToggle 聚焦时不编辑)。
                                // 端口只收数字;URL 收 URL 合法字符。方向/Home/End/Backspace 通用。
                                if focus == MqttFocus::Tcp
                                    || focus == MqttFocus::Ws
                                    || focus == MqttFocus::Url
                                {
                                    let req = if bytes == b"\x1b[D" {
                                        Some(InputRequest::GoToPrevChar)
                                    } else if bytes == b"\x1b[C" {
                                        Some(InputRequest::GoToNextChar)
                                    } else if bytes == b"\x1b[H" {
                                        Some(InputRequest::GoToStart)
                                    } else if bytes == b"\x1b[F" {
                                        Some(InputRequest::GoToEnd)
                                    } else if bytes == [0x08] || bytes == [0x7f] {
                                        Some(InputRequest::DeletePrevChar)
                                    } else if bytes.len() == 1 {
                                        let c = bytes[0];
                                        let is_url = focus == MqttFocus::Url;
                                        let allowed = if is_url {
                                            c.is_ascii_alphanumeric()
                                                || matches!(
                                                    c,
                                                    b':' | b'/' | b'.' | b'_' | b'-' | b'~' | b'@'
                                                )
                                        } else {
                                            c.is_ascii_digit()
                                        };
                                        let cur_len = match focus {
                                            MqttFocus::Ws => ws.value().chars().count(),
                                            MqttFocus::Tcp => tcp.value().chars().count(),
                                            MqttFocus::Url => url.value().chars().count(),
                                            _ => 0,
                                        };
                                        let cap = if is_url { 60 } else { 5 };
                                        if allowed && cur_len < cap {
                                            Some(InputRequest::InsertChar(c as char))
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    if let Some(req) = req {
                                        match focus {
                                            MqttFocus::Ws => {
                                                ws.handle(req);
                                            }
                                            MqttFocus::Tcp => {
                                                tcp.handle(req);
                                            }
                                            MqttFocus::Url => {
                                                url.handle(req);
                                            }
                                            MqttFocus::BrokerStart | MqttFocus::ClientToggle => {}
                                        }
                                    }
                                }
                                ModalState::MqttPanel {
                                    tcp,
                                    ws,
                                    url,
                                    focus,
                                }
                            }
                        }
                        ModalState::Error(_) => ModalState::None, // 任意键关闭
                        ModalState::None => ModalState::None,
                    };
                    modal = next;
                    let screen = Arc::new(vt_parser.screen().clone());
                    redraw(
                        &screen,
                        ui_title,
                        &http,
                        mqtt_broker_on,
                        mqtt_client.is_some(),
                        &modal,
                    );
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Click { col, row }) => {
                // 模态打开时忽略点击。
                if matches!(modal, ModalState::None) {
                    let area = Rect::new(0, 0, term_size.0, term_size.1);
                    // 退出按钮(右):发 0x03 + 退出循环,等同 Ctrl+C。
                    let quit = quit_button_rect(area, "Quit");
                    if col >= quit.x
                        && col < quit.x + quit.width
                        && row >= quit.y
                        && row < quit.y + quit.height
                    {
                        log::info!("Quit button clicked, sending Ctrl+C (0x03)");
                        terminal.send_bytes(&[0x03]).await?;
                        break;
                    }
                    // HTTP 按钮(左,off 态):打开地址输入框。
                    if matches!(http, HttpBtnState::Off) {
                        let btn = http_button_rect(area, "HTTP off");
                        if col >= btn.x
                            && col < btn.x + btn.width
                            && row >= btn.y
                            && row < btn.y + btn.height
                        {
                            modal = ModalState::PortInput {
                                input: tui_input::Input::new(default_bind.clone()),
                            };
                            let screen = Arc::new(vt_parser.screen().clone());
                            redraw(
                                &screen,
                                ui_title,
                                &http,
                                mqtt_broker_on,
                                mqtt_client.is_some(),
                                &modal,
                            );
                        }
                    }
                    // MQTT 按钮(HTTP 右边):只要配了 [mqtt] 就开控制面板(起停都在面板里,不看 off 态)。
                    if mqtt_cfg_present {
                        let hl = button_label("HTTP", &http);
                        let mlabel = mqtt_footer_label(&MqttButtonsState {
                            broker_on: mqtt_broker_on,
                            client_on: mqtt_client.is_some(),
                        });
                        let mbtn = mqtt_button_rect(area, &hl, &mlabel);
                        if col >= mbtn.x
                            && col < mbtn.x + mbtn.width
                            && row >= mbtn.y
                            && row < mbtn.y + mbtn.height
                        {
                            let cfg = mqtt_cfg
                                .as_ref()
                                .expect("mqtt_cfg_present implies mqtt_cfg");
                            modal = ModalState::MqttPanel {
                                tcp: tui_input::Input::new(cfg.builtin_port.to_string()),
                                ws: tui_input::Input::new(cfg.builtin_ws_port.to_string()),
                                // 预填生效 URL(见 MqttConfig::for_client):填了 broker 就用配置值,
                                // 没填 + 内置 broker 才默认本地。
                                url: tui_input::Input::new(cfg.for_client().broker),
                                focus: MqttFocus::Tcp,
                            };
                            let screen = Arc::new(vt_parser.screen().clone());
                            redraw(
                                &screen,
                                ui_title,
                                &http,
                                mqtt_broker_on,
                                mqtt_client.is_some(),
                                &modal,
                            );
                        }
                    }
                }
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollUp { rows })
            | TerminalEvent::Input(ClientMessage::ScrollUp { rows }) => {
                let delta = scroll_delta(rows, page_rows);
                log::info!("ScrollUp rows={rows} -> delta={delta}");
                let s = vt_parser.screen().scrollback();
                vt_parser
                    .screen_mut()
                    .set_scrollback(s.saturating_add(delta));
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                );

                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollDown { rows })
            | TerminalEvent::Input(ClientMessage::ScrollDown { rows }) => {
                let delta = scroll_delta(rows, page_rows);
                log::info!("ScrollDown rows={rows} -> delta={delta}");
                let s = vt_parser.screen().scrollback();
                vt_parser
                    .screen_mut()
                    .set_scrollback(s.saturating_sub(delta));
                let screen = Arc::new(vt_parser.screen().clone());
                redraw(
                    &screen,
                    ui_title,
                    &http,
                    mqtt_broker_on,
                    mqtt_client.is_some(),
                    &modal,
                );

                send_screen(&tx, screen);
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
                );
                send_screen(&tx, screen);
            }
            TerminalEvent::Input(ClientMessage::Sync { width, height }) => {
                // sync 只决定宽度:按客户端像素宽换算列数;高度(行数)保持当前不变。
                // 但高度用来记「一页」行数(= 能塞下 − 1),供 scroll rows=0 时用。
                page_rows = page_rows_from_height(height);
                let avail_w = (width as u32).saturating_sub(2 * SCREEN_PADDING);
                let cols = (avail_w / SCREEN_CHAR_WIDTH).max(8) as u16; // 最低 8 列
                // 列数和当前一致就不 resize(避免抖屏);但 sync 本身也是「请求刷屏」,仍回送屏幕。
                let (rows, cur_cols) = vt_parser.screen().size(); // rows = 原始高度,保留
                if cur_cols != cols {
                    log::info!(
                        "Sync: {width}px wide -> resize PTY cols {cur_cols} -> {cols} (rows kept {rows})"
                    );
                    vt_parser.screen_mut().set_size(rows, cols);
                    let _ = terminal.resize(rows, cols);
                } else {
                    log::debug!("Sync: cols unchanged ({cols}), skip resize");
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
    window_size: Option<(u16, u16)>, // (width, height)
    window_h_offset: &mut u16,
    format: crate::protocol::ImageFormat,
    target_height: Option<u16>, // 把图片高度补齐到该值(不足补到该值,超过补到 8 的倍数)
) -> anyhow::Result<Vec<u8>> {
    let config = crate::screenshot::ScreenshotConfig {
        show_decorations: false,
        ..Default::default()
    };

    let image = crate::screenshot::capture_screen(screen, &config)
        .map_err(|e| anyhow::anyhow!("Failed to capture screen: {}", e))?;

    let mut dyn_image = image::DynamicImage::ImageRgba8(image);
    let is_png = matches!(format, crate::protocol::ImageFormat::Png);

    if let Some((width, height)) = window_size {
        let orig_width = dyn_image.width();
        let orig_height = dyn_image.height();
        log::debug!(
            "Original image size: {}x{}, requested window size: {}x{}",
            orig_width,
            orig_height,
            width,
            height
        );
        let scale = width as f32 / orig_width as f32;
        let new_height = (orig_height as f32 * scale).round() as u32;

        dyn_image = image::DynamicImage::ImageRgba8(image::imageops::resize(
            &dyn_image,
            width as u32,
            new_height,
            image::imageops::FilterType::Lanczos3,
        ));

        // 根据垂直偏移截取
        let crop_height = height as u32;
        if *window_h_offset == u16::MAX {
            *window_h_offset = (new_height - crop_height) as u16;
        }

        let y_offset = *window_h_offset as u32;

        if crop_height > new_height {
            // 在顶部填充缺失的部分（图片高度不足）
            log::debug!(
                "Padding image: crop_height {} > new_height {}, padding {} pixels at top",
                crop_height,
                new_height,
                crop_height - new_height
            );
            let padding_top = crop_height - new_height;

            if is_png {
                let mut padded = image::RgbaImage::new(width as u32, crop_height);
                for pixel in padded.pixels_mut() {
                    *pixel = image::Rgba([30, 30, 30, 255]);
                }
                image::imageops::overlay(&mut padded, &dyn_image.to_rgba8(), 0, padding_top as i64);
                dyn_image = image::DynamicImage::ImageRgba8(padded);
            } else {
                let mut padded = image::RgbImage::new(width as u32, crop_height);
                for pixel in padded.pixels_mut() {
                    *pixel = image::Rgb([30, 30, 30]);
                }
                image::imageops::overlay(&mut padded, &dyn_image.to_rgb8(), 0, padding_top as i64);
                dyn_image = image::DynamicImage::ImageRgb8(padded);
            }
        } else if y_offset + crop_height > new_height {
            // 偏移量过大，调整到对齐底部
            log::warn!(
                "Vertical offset {} + crop height {} exceeds image height {}, adjusting offset",
                y_offset,
                crop_height,
                new_height
            );
            *window_h_offset = (new_height - crop_height) as u16;
            return Ok(Vec::new());
        } else {
            // 正常截取
            dyn_image =
                image::imageops::crop(&mut dyn_image, 0, y_offset, width as u32, crop_height)
                    .to_image()
                    .into();
        }
    }

    // 按目标高度补齐图片高度(主要用于 MQTT 出站):不足 sync 高度就补到该高度,
    // 超过就补到 8 的倍数。内容保持在上,下方用背景色填。
    if let Some(sync_h) = target_height {
        let cur_h = dyn_image.height();
        let target = if cur_h < sync_h as u32 {
            sync_h as u32
        } else {
            cur_h.div_ceil(8) * 8
        };
        if target > cur_h {
            let mut padded = image::RgbaImage::new(dyn_image.width(), target);
            for p in padded.pixels_mut() {
                *p = image::Rgba([30, 30, 30, 255]);
            }
            image::imageops::overlay(&mut padded, &dyn_image.to_rgba8(), 0, 0);
            dyn_image = image::DynamicImage::ImageRgba8(padded);
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
