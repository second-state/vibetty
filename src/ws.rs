use std::sync::Arc;

use axum::{extract::State, response::IntoResponse};
use tokio::sync::{broadcast, mpsc};
use vt100::Callbacks;

use crate::protocol::{ClientMessage, ServerMessage};

/// Image broadcast frame interval (ms)
const IMAGE_FRAME_INTERVAL_MS: u64 = 300;
/// 终端截图里单个字符单元格的像素宽。`vibetty-screenshot` 在 font_size=14.0 +
/// 内嵌字体 Sarasa Mono SC Light(swash 后端)下由 `get_char_metrics(14.0)` 实测得到。
/// 客户端 Sync 发的是【像素】,要除以它换算成 PTY 列数。
/// ⚠️ 改 `render_screen_to_image` 的 font_size 或换字体时必须同步更新。
pub(crate) const SCREEN_CHAR_WIDTH: u32 = 8;
/// 单个字符单元格的像素高,同上(`get_char_metrics(14.0)` 返回 `(8, 18)`)。
/// 记录用:sync 现在 height 保留原始行数、不再按高度换算,故暂未被引用。
#[allow(dead_code)]
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

type ClientTx = mpsc::Sender<ClientMessage>;
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
    pub tx: ServerTx,
    pub cli_tx: ClientTx,
    pub screenshot_tx: ScreenshotTx,
    pub image_format: crate::protocol::ImageFormat,
}

fn send_screen(tx: &ServerTx, screen: Arc<vt100::Screen>) {
    let _ = tx.send(ServerMessage::Screen(screen));
}

#[allow(clippy::too_many_arguments)]
pub async fn run_command(
    command: Vec<String>,
    mut rx: ClientRx,
    ui_rx: &mut mpsc::Receiver<crate::ui::UIEvent>,
    tx: ServerTx,
    listen_port: u16,
    mut screenshot_rx: mpsc::Receiver<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    tui: &mut crate::ui::TuiTerminal,
    ui_title: &mut String,
    ui_footer: &str,
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
        &[("VIBETTY_PORT".to_string(), listen_port.to_string())],
        (vt_rows, vt_cols),
    )
    .await?;

    let mut vt_parser =
        vt100::Parser::new_with_callbacks(vt_rows, vt_cols, 8096, WindowCallbacks::new());

    // Frame rate limit for image broadcast (default 2 fps)
    let frame_interval = std::time::Duration::from_millis(IMAGE_FRAME_INTERVAL_MS);
    let mut last_frame_time = std::time::Instant::now() - frame_interval;

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
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
                if tx
                    .send(ServerMessage::PtyOutput(output.into_bytes()))
                    .is_err()
                {
                    log::warn!("[{}] no active PTY subscribers", terminal.session_id());
                    continue;
                }

                // Generate JPEG and broadcast chunks for img subscribers (rate limited)
                let now = std::time::Instant::now();
                if now.duration_since(last_frame_time) >= frame_interval || title.starts_with("✳")
                {
                    last_frame_time = now;
                    send_screen(&tx, screen);
                }
            }

            TerminalEvent::UIEvent(crate::ui::UIEvent::Input(input)) => {
                log::info!("UI Input: {:?}", String::from_utf8_lossy(&input));
                terminal.send_bytes(&input).await?;
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ResizePtyWidth(i)) => {
                log::info!("UI ResizePtyWidth: {}", i);
                let (rows, cols) = vt_parser.screen().size();
                let new_cols = (cols as i16 + i).max(10) as u16;
                vt_parser.screen_mut().set_size(rows, new_cols);
                let _ = terminal.resize(rows, new_cols);
                log::debug!("Resized PTY to {} rows and {} cols", rows, new_cols);
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollUp)
            | TerminalEvent::Input(ClientMessage::ScrollUp) => {
                log::info!("ScrollUp");
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s + 5);
                let screen = Arc::new(vt_parser.screen().clone());
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));

                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::ScrollDown)
            | TerminalEvent::Input(ClientMessage::ScrollDown) => {
                log::info!("ScrollDown");
                let s = vt_parser.screen().scrollback();
                vt_parser.screen_mut().set_scrollback(s.saturating_sub(5));
                let screen = Arc::new(vt_parser.screen().clone());
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));

                send_screen(&tx, screen);
            }
            TerminalEvent::UIEvent(crate::ui::UIEvent::Resize(cols, rows)) => {
                log::info!("Resize: cols={}, rows={}", cols, rows);
                let vt_cols = cols.saturating_sub(TUI_COLS_PADDING);
                let vt_rows = rows.saturating_sub(TUI_ROWS_PADDING);
                vt_parser.screen_mut().set_size(vt_rows, vt_cols);
                let _ = terminal.resize(vt_rows, vt_cols);
                let screen = Arc::new(vt_parser.screen().clone());
                send_screen(&tx, screen);
            }
            TerminalEvent::Input(ClientMessage::Sync { width, .. }) => {
                // sync 只决定宽度:按客户端像素宽换算列数;高度(行数)保持当前不变。
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
                let title = ui_title.clone();
                let footer = ui_footer.to_string();
                let _ =
                    tui.draw(|f| crate::ui::render_frame(f, &screen, &title, "Vibetty", &footer));
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
