use std::io;

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tui_term::widget::PseudoTerminal;

/// 鼠标滚轮每格滚动的行数(客户端的 scroll 可自带 rows;0 = 整页)。
const MOUSE_SCROLL_ROWS: u16 = 3;

pub enum UIEvent {
    Input(Vec<u8>),
    /// 鼠标左键点击(col/row 为终端单元格坐标)。
    Click {
        col: u16,
        row: u16,
    },
    ScrollUp {
        rows: u16,
    },
    ScrollDown {
        rows: u16,
    },
    Resize(u16, u16),
}

/// footer HTTP 按钮的显示状态。
#[derive(Default)]
pub(crate) enum HttpBtnState {
    #[default]
    Off,
    On(String),
}

/// footer MQTT 按钮的显示状态(broker / client 各一个开关),也供 MqttPanel 渲染状态用。
#[derive(Clone, Copy, Default)]
pub(crate) struct MqttButtonsState {
    pub broker_on: bool,
    pub client_on: bool,
}

/// MqttPanel 对话框聚焦的项(↑↓/Tab 在各项间循环)。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum MqttFocus {
    Tcp,
    Ws,
    BrokerStart,
    Url,
    ClientToggle,
}

/// 端口输入对话框的状态(打开时键盘事件路由给它,不进 PTY)。
#[derive(Default)]
pub(crate) enum ModalState {
    #[default]
    None,
    PortInput {
        /// 绑定地址输入(含光标状态),光标/插入/删除由 tui-input 管理。
        input: tui_input::Input,
    },
    /// MQTT 控制面板:上块 broker(端口 + Start)、下块 client(URL + Start/Stop)。
    /// broker/client 是否在跑由 `render_frame` 的 `mqtt` 参数提供,本结构只管表单 + 聚焦。
    MqttPanel {
        tcp: tui_input::Input,
        ws: tui_input::Input,
        /// client 要连的 broker URL(Enter 存回 `[mqtt] broker`)。
        url: tui_input::Input,
        /// 当前聚焦的项。
        focus: MqttFocus,
    },
    Error(String),
}

pub type UITx = mpsc::Sender<UIEvent>;

pub type TuiTerminal = Terminal<CrosstermBackend<std::io::Stdout>>;

pub fn init_terminal() -> io::Result<TuiTerminal> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

pub fn cleanup_terminal(terminal: &mut TuiTerminal) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()
}

pub fn render_frame(
    f: &mut Frame,
    screen: &vt100::Screen,
    title: &str,
    header_text: &str,
    http: &HttpBtnState,
    mqtt: Option<&MqttButtonsState>,
    modal: &ModalState,
) {
    let size = f.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(size);

    let header_display = if title.is_empty() {
        header_text.to_string()
    } else {
        format!("{} - {}", header_text, title)
    };
    let header = Paragraph::new(header_display)
        .block(Block::new().borders(Borders::ALL))
        .alignment(Alignment::Center);
    f.render_widget(header, chunks[0]);

    {
        let (term_rows, term_cols) = screen.size();
        // 终端屏幕有固定的 cols×rows,可能小于 pane(如 ESP32 sync 把 PTY resize 成小屏后,
        // 本地 TUI pane 仍很大)。按屏幕尺寸(+边框各 1)在 pane 内居中;屏幕大于 pane 时铺满并裁切。
        // 先 Clear 整个 pane:居中的终端只重绘自己的 rect,不清的话 pane 其余区域会残留上一帧
        // (更宽时的)旧内容,滚动时尤其明显,看着像重复行。
        f.render_widget(Clear, chunks[1]);
        let area = centered_rect(
            chunks[1],
            term_cols.saturating_add(2),
            term_rows.saturating_add(2),
        );
        let pseudo_term = PseudoTerminal::new(screen).block(Block::new().borders(Borders::ALL));
        f.render_widget(pseudo_term, area);
    }

    // footer:左 HTTP 开关(+ MQTT 开关在其右)+ 右退出按钮。
    {
        let http_label = button_label("HTTP", http);
        render_button(f, http_button_rect(f.area(), &http_label), &http_label);
        if let Some(mqtt) = mqtt {
            let mqtt_label = mqtt_footer_label(mqtt);
            render_button(
                f,
                mqtt_button_rect(f.area(), &http_label, &mqtt_label),
                &mqtt_label,
            );
        }
        render_button(f, quit_button_rect(f.area(), "Quit"), "Quit");
    }

    // 模态对话框(输入地址 / 显示错误),覆盖在终端之上。
    if !matches!(modal, ModalState::None) {
        let h = if matches!(modal, ModalState::MqttPanel { .. }) {
            16
        } else {
            9
        };
        let area = centered_rect(f.area(), 48, h);
        f.render_widget(Clear, area);
        let title = match modal {
            ModalState::PortInput { .. } => " HTTP Server ",
            ModalState::MqttPanel { .. } => " MQTT ",
            ModalState::Error(_) => " Error ",
            ModalState::None => unreachable!(),
        };
        let block = Block::new().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        // 主内容区 + 底部提示行。
        let panes = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(inner);
        let hint: String = match modal {
            ModalState::PortInput { .. } => "Enter to start · Esc to cancel".to_string(),
            // Enter 的提示随当前聚焦项变化(且 Start/Stop 只显示当前会做的那一个动作)。
            ModalState::MqttPanel { focus, .. } => {
                let st = mqtt.copied().unwrap_or_default();
                let enter = match focus {
                    MqttFocus::Tcp | MqttFocus::Ws => "Enter saves port",
                    MqttFocus::Url => "Enter saves URL",
                    MqttFocus::BrokerStart if st.broker_on => "broker already running",
                    MqttFocus::BrokerStart => "Enter starts broker",
                    MqttFocus::ClientToggle if st.client_on => "Enter stops client",
                    MqttFocus::ClientToggle => "Enter starts client",
                };
                format!("↑↓/Tab move · {enter} · Esc")
            }
            ModalState::Error(_) => "(press any key to close)".to_string(),
            ModalState::None => unreachable!(),
        };
        match modal {
            ModalState::PortInput { input } => {
                // 输入行竖直居中。
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(0),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .split(panes[0]);
                render_input_field(f, rows[1], "Bind:", input, true);
            }
            ModalState::MqttPanel {
                tcp,
                ws,
                url,
                focus,
            } => {
                // 上下两块:broker(上)+ client(下),各一个带边框 + 标题的子 block。
                let st = mqtt.copied().unwrap_or_default();
                let sections = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(7), Constraint::Length(4)])
                    .split(panes[0]);
                // --- Broker 块 ---
                let broker_block = Block::new().borders(Borders::ALL).title(" Broker ");
                let broker_inner = broker_block.inner(sections[0]);
                f.render_widget(broker_block, sections[0]);
                let brows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Min(0),
                    ])
                    .split(broker_inner);
                render_input_field(f, brows[0], "TCP:", tcp, *focus == MqttFocus::Tcp);
                render_input_field(f, brows[1], "WS :", ws, *focus == MqttFocus::Ws);
                let broker_text = if st.broker_on {
                    format!("● broker running :{}", tcp.value().trim())
                } else {
                    "Start broker".to_string()
                };
                let broker_style = if st.broker_on {
                    Style::default().fg(Color::Green)
                } else if *focus == MqttFocus::BrokerStart {
                    Style::default().bg(Color::LightBlue).fg(Color::Black)
                } else {
                    Style::default()
                };
                f.render_widget(
                    Paragraph::new(broker_text)
                        .alignment(Alignment::Center)
                        .style(broker_style),
                    brows[2],
                );
                // --- Client 块:URL 字段(上)+ Start/Stop(下,单行) ---
                let client_block = Block::new().borders(Borders::ALL).title(" Client ");
                let client_inner = client_block.inner(sections[1]);
                f.render_widget(client_block, sections[1]);
                let crows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(1), Constraint::Length(1)])
                    .split(client_inner);
                render_input_field(f, crows[0], "URL:", url, *focus == MqttFocus::Url);
                let client_text = if st.client_on {
                    "Stop client".to_string()
                } else {
                    "Start client".to_string()
                };
                let client_style = if *focus == MqttFocus::ClientToggle {
                    Style::default().bg(Color::LightBlue).fg(Color::Black)
                } else {
                    Style::default()
                };
                f.render_widget(
                    Paragraph::new(client_text)
                        .alignment(Alignment::Center)
                        .style(client_style),
                    crows[1],
                );
            }
            ModalState::Error(msg) => {
                f.render_widget(
                    Paragraph::new(msg.clone()).alignment(Alignment::Center),
                    panes[0],
                );
            }
            ModalState::None => unreachable!(),
        }
        f.render_widget(Paragraph::new(hint).alignment(Alignment::Center), panes[1]);
    }
}

/// 在 `area` 内居中放置一个 `want_w` × `want_h` 的矩形;超出 `area` 时裁到 `area` 大小。
fn centered_rect(area: Rect, want_w: u16, want_h: u16) -> Rect {
    let w = want_w.min(area.width);
    let h = want_h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect::new(x, y, w, h)
}

/// 画一个带边框、文字居中的按钮。
fn render_button(f: &mut Frame, area: Rect, label: &str) {
    let block = Block::new().borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(label.to_string()).alignment(Alignment::Center),
        inner,
    );
}

/// 画一个输入字段:[固定宽度标签][整块背景高亮的输入框]。
/// `focused` 时显示高亮光标单元格(末尾则高亮一个空格);否则只显示值。
fn render_input_field(
    f: &mut Frame,
    area: Rect,
    label: &str,
    input: &tui_input::Input,
    focused: bool,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(6), Constraint::Min(0)])
        .split(area);
    f.render_widget(Paragraph::new(label).alignment(Alignment::Right), cols[0]);
    let val = input.value();
    let cur = input.cursor().min(val.chars().count());
    let byte = val.char_indices().nth(cur).map_or(val.len(), |(i, _)| i);
    let field = Style::default().bg(Color::DarkGray);
    let line = if focused {
        let (at, after) = if byte < val.len() {
            let next = val
                .char_indices()
                .nth(cur + 1)
                .map_or(val.len(), |(i, _)| i);
            (&val[byte..next], &val[next..])
        } else {
            (" ", "")
        };
        let cursor_cell = Style::default().bg(Color::LightBlue).fg(Color::Black);
        Line::from(vec![
            Span::styled(&val[..byte], field),
            Span::styled(at, cursor_cell),
            Span::styled(after, field),
        ])
    } else {
        Line::from(vec![Span::styled(val, field)])
    };
    f.render_widget(Paragraph::new(line).style(field), cols[1]);
}

/// 按钮文字:`{prefix} off` 或 `{prefix} {value}`。
pub(crate) fn button_label(prefix: &str, state: &HttpBtnState) -> String {
    match state {
        HttpBtnState::Off => format!("{prefix} off"),
        HttpBtnState::On(v) => format!("{prefix} {v}"),
    }
}

/// footer MQTT 按钮文字:据 broker/client 两个开关组合出简洁状态(render 与点击命中检测共用)。
pub(crate) fn mqtt_footer_label(mqtt: &MqttButtonsState) -> String {
    match (mqtt.broker_on, mqtt.client_on) {
        (false, false) => "MQTT off".to_string(),
        (true, false) => "MQTT brkr".to_string(),
        (false, true) => "MQTT conn".to_string(),
        (true, true) => "MQTT on".to_string(),
    }
}

/// footer 左侧「HTTP 按钮」的 Rect(宽度随 `label` 内容变化,含两侧边框)。
/// `render_frame` 画按钮、`run_command` 命中检测共用,保证点击坐标与渲染位置一致;
/// 内部 Layout 必须与 `render_frame` 完全相同。
pub(crate) fn http_button_rect(frame_area: Rect, label: &str) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame_area);
    let footer = chunks[2];
    let want = label.chars().count() as u16 + 2; // +2:左右边框
    let width = want.min(footer.width);
    Rect::new(footer.x, footer.y, width, footer.height)
}

/// footer 中「MQTT 按钮」的 Rect,紧跟 HTTP 按钮右侧(间隔 1 格)。
/// render 画按钮、`run_command` 命中检测共用。
pub(crate) fn mqtt_button_rect(frame_area: Rect, http_label: &str, mqtt_label: &str) -> Rect {
    let http = http_button_rect(frame_area, http_label);
    let width = mqtt_label.chars().count() as u16 + 2; // +2:左右边框
    Rect::new(http.x + http.width + 1, http.y, width, http.height)
}

/// footer 右侧「退出按钮」的 Rect(右对齐,宽度随 `label` 变化,含两侧边框)。
/// render 画按钮、`run_command` 命中检测共用;Layout 与 `render_frame` 相同。
pub(crate) fn quit_button_rect(frame_area: Rect, label: &str) -> Rect {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame_area);
    let footer = chunks[2];
    let want = label.chars().count() as u16 + 2; // +2:左右边框
    let width = want.min(footer.width);
    let x = footer.x + footer.width - width;
    Rect::new(x, footer.y, width, footer.height)
}

pub fn spawn_event_loop(ui_tx: UITx) {
    let _thread = std::thread::spawn(move || {
        if let Err(e) = event_loop_thread(ui_tx) {
            log::error!("Event loop error: {}", e);
        }
    });
}

fn event_loop_thread(tx_to_pty: UITx) -> anyhow::Result<()> {
    let timeout = Duration::from_millis(500);
    loop {
        if event::poll(timeout)? {
            let evt = event::read()?;

            if let Event::Key(key) = evt {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = tx_to_pty.blocking_send(UIEvent::Input(Vec::from(&[0x03][..])));
                            return Err(anyhow::anyhow!("Received Ctrl+C, exiting event loop"));
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = tx_to_pty.blocking_send(UIEvent::Input(Vec::from(&[0x04][..])));
                        }
                        _ => {
                            if let Some(bytes) = bytes_from_key(key) {
                                let _ = tx_to_pty.blocking_send(UIEvent::Input(bytes));
                            }
                        }
                    }
                }
            } else if let Event::Paste(s) = evt {
                let bytes = s.into_bytes();
                let _ = tx_to_pty.blocking_send(UIEvent::Input(bytes));
            } else if let Event::Mouse(mouse) = evt {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        let _ = tx_to_pty.blocking_send(UIEvent::ScrollUp {
                            rows: MOUSE_SCROLL_ROWS,
                        });
                    }
                    MouseEventKind::ScrollDown => {
                        let _ = tx_to_pty.blocking_send(UIEvent::ScrollDown {
                            rows: MOUSE_SCROLL_ROWS,
                        });
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let _ = tx_to_pty.blocking_send(UIEvent::Click {
                            col: mouse.column,
                            row: mouse.row,
                        });
                    }
                    _ => {}
                }
            } else if let Event::Resize(cols, rows) = evt {
                let _ = tx_to_pty.blocking_send(UIEvent::Resize(cols, rows));
            }
        }
    }
}

fn bytes_from_key(key: KeyEvent) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();

    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let b = (c.to_ascii_uppercase() as u8).saturating_sub(b'A' - 1);
                bytes.push(b);
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                bytes.extend_from_slice(&[0x1b, c as u8]);
            } else {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                bytes.extend_from_slice(encoded.as_bytes());
            }
        }
        KeyCode::Enter => bytes.push(b'\r'),
        KeyCode::Tab => {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                bytes.extend_from_slice(b"\x1b[Z");
            } else {
                bytes.push(b'\t');
            }
        }
        KeyCode::BackTab => bytes.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => bytes.push(0x08),
        KeyCode::Esc => bytes.push(0x1b),
        KeyCode::Up => bytes.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => bytes.extend_from_slice(b"\x1b[B"),
        KeyCode::Left => bytes.extend_from_slice(b"\x1b[D"),
        KeyCode::Right => bytes.extend_from_slice(b"\x1b[C"),
        KeyCode::Home => bytes.extend_from_slice(b"\x1b[H"),
        KeyCode::End => bytes.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => bytes.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => bytes.extend_from_slice(b"\x1b[6~"),
        KeyCode::Delete => bytes.extend_from_slice(b"\x1b[3~"),
        KeyCode::Insert => bytes.extend_from_slice(b"\x1b[2~"),
        KeyCode::F(1) => bytes.extend_from_slice(b"\x1bOP"),
        KeyCode::F(2) => bytes.extend_from_slice(b"\x1bOQ"),
        KeyCode::F(3) => bytes.extend_from_slice(b"\x1bOR"),
        KeyCode::F(4) => bytes.extend_from_slice(b"\x1bOS"),
        KeyCode::F(5) => bytes.extend_from_slice(b"\x1b[15~"),
        KeyCode::F(6) => bytes.extend_from_slice(b"\x1b[17~"),
        KeyCode::F(7) => bytes.extend_from_slice(b"\x1b[18~"),
        KeyCode::F(8) => bytes.extend_from_slice(b"\x1b[19~"),
        KeyCode::F(9) => bytes.extend_from_slice(b"\x1b[20~"),
        KeyCode::F(10) => bytes.extend_from_slice(b"\x1b[21~"),
        KeyCode::F(11) => bytes.extend_from_slice(b"\x1b[23~"),
        KeyCode::F(12) => bytes.extend_from_slice(b"\x1b[24~"),
        _ => return None,
    }

    Some(bytes)
}
