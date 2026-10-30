use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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

use crate::terminal::agent::AgentState;

pub enum UIEvent {
    Input(Vec<u8>),
    Resize(u16, u16),
    /// 菜单快捷键(config `[ui] menu_key`,默认 ctrl+shift+p):开关主菜单弹窗。
    /// 在事件循环拦截,不转发 PTY。
    ToggleMenu,
}

/// menu_key 的默认值,写进文档和 config 解析失败时的兜底。
pub const DEFAULT_MENU_KEY: &str = "ctrl+shift+p";

/// 解析后的菜单快捷键(与 crossterm 的 KeyEvent 精确比对)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding {
    pub modifiers: KeyModifiers,
    pub code: KeyCode,
}

/// 解析 `ctrl+shift+p` 这类快捷键描述:`+` 分隔修饰键(ctrl/shift/alt/super)+ 单个字符键。
/// 解析不了返回 None(调用方回退默认值)。
pub fn parse_keybinding(s: &str) -> Option<KeyBinding> {
    let mut modifiers = KeyModifiers::empty();
    let mut key: Option<char> = None;
    for part in s.split('+') {
        match part.trim().to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "alt" | "opt" | "option" => modifiers |= KeyModifiers::ALT,
            "super" | "cmd" | "meta" => modifiers |= KeyModifiers::SUPER,
            other if other.chars().count() == 1 => key = Some(other.chars().next()?),
            _ => return None,
        }
    }
    let code = KeyCode::Char(key?);
    Some(KeyBinding { modifiers, code })
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

impl MqttFocus {
    /// 下一项(Tab / ↓):Tcp → Ws → BrokerStart → Url → ClientToggle → Tcp。
    pub(crate) fn next(self) -> Self {
        match self {
            MqttFocus::Tcp => MqttFocus::Ws,
            MqttFocus::Ws => MqttFocus::BrokerStart,
            MqttFocus::BrokerStart => MqttFocus::Url,
            MqttFocus::Url => MqttFocus::ClientToggle,
            MqttFocus::ClientToggle => MqttFocus::Tcp,
        }
    }

    /// 上一项(↑):反向循环。
    pub(crate) fn prev(self) -> Self {
        match self {
            MqttFocus::Tcp => MqttFocus::ClientToggle,
            MqttFocus::Ws => MqttFocus::Tcp,
            MqttFocus::BrokerStart => MqttFocus::Ws,
            MqttFocus::Url => MqttFocus::BrokerStart,
            MqttFocus::ClientToggle => MqttFocus::Url,
        }
    }
}

/// 菜单弹窗(菜单快捷键)里可聚焦的条目。↑↓/Tab 循环,Enter 执行。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuFocus {
    Mqtt,
    Border,
    Fit,
    Quit,
}

impl MenuFocus {
    /// 下一项(↓/Tab):循环。`has_mqtt=false` 时跳过 Mqtt 条目(未配 [mqtt] 不可用)。
    pub(crate) fn next(self, has_mqtt: bool) -> Self {
        let mut f = match self {
            MenuFocus::Mqtt => MenuFocus::Border,
            MenuFocus::Border => MenuFocus::Fit,
            MenuFocus::Fit => MenuFocus::Quit,
            MenuFocus::Quit => MenuFocus::Mqtt,
        };
        if !has_mqtt && f == MenuFocus::Mqtt {
            f = MenuFocus::Fit;
        }
        f
    }

    /// 上一项(↑):反向循环。
    pub(crate) fn prev(self, has_mqtt: bool) -> Self {
        let mut f = match self {
            MenuFocus::Mqtt => MenuFocus::Quit,
            MenuFocus::Border => MenuFocus::Mqtt,
            MenuFocus::Fit => MenuFocus::Border,
            MenuFocus::Quit => MenuFocus::Fit,
        };
        if !has_mqtt && f == MenuFocus::Mqtt {
            f = MenuFocus::Quit;
        }
        f
    }
}

/// 模态对话框的状态(打开时键盘事件路由给它,不进 PTY)。
/// MqttPanel 三个 tui_input 比其它变体大很多,装箱得不偿失(整体 Copy/移动成本低)。
#[allow(clippy::large_enum_variant)]
#[derive(Default)]
pub(crate) enum ModalState {
    #[default]
    None,
    /// 主菜单弹窗(Ctrl+Shift+P 开关):原顶部按钮行的全部入口收在这里。
    Menu {
        focus: MenuFocus,
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
    // 开 kitty 键盘协议(仅 disambiguate 位):Ctrl+Shift+字母 这类没有 legacy 编码的
    // 组合会按 CSI-u 上报,crossterm 才能解出 CONTROL|SHIFT 修饰键(供 Ctrl+Shift+P
    // 快捷键)。不支持的终端忽略该序列,只是快捷键不触发,无副作用。
    // 不开鼠标捕捉:点击/滚轮不再上报,vibetty 不支持鼠标交互;滚轮交给宿主终端
    // 自己滚本地 scrollback。快捷键修饰键仍靠 kitty 协议(disambiguate 位)。
    execute!(
        stdout,
        EnterAlternateScreen,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

pub fn cleanup_terminal(terminal: &mut TuiTerminal) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        PopKeyboardEnhancementFlags
    )?;
    terminal.show_cursor()
}

/// 按需开/关对宿主终端的鼠标上报(?1002 按键事件 + ?1006 SGR 编码)。
/// 由 ws 主循环在检测到子进程 DECSET 鼠标模式时调用——只有内部程序自己要鼠标,
/// 我们才捕获并透传;普通 shell 不请求,点击就不会有转义序列灌进 PTY。
pub fn set_mouse_capture(on: bool) {
    use std::io::Write;
    let seq: &[u8] = if on {
        b"\x1b[?1002h\x1b[?1006h"
    } else {
        b"\x1b[?1002l\x1b[?1006l"
    };
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(seq);
    let _ = out.flush();
}

/// 把 crossterm 鼠标事件编码成 SGR (1006) 字节序列,原样转发进 PTY。
fn encode_mouse_sgr(m: &crossterm::event::MouseEvent) -> Option<Vec<u8>> {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut code = match m.kind {
        MouseEventKind::ScrollUp => 64,
        MouseEventKind::ScrollDown => 65,
        MouseEventKind::Down(b) | MouseEventKind::Up(b) | MouseEventKind::Drag(b) => match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
        },
        _ => return None, // Moved(1003 模式)等不透传
    };
    // 拖动 +32;修饰键 shift/alt/ctrl = +4/+8/+16(xterm 规范)。
    let release = matches!(m.kind, MouseEventKind::Up(_));
    if matches!(m.kind, MouseEventKind::Drag(_)) {
        code += 32;
    }
    if m.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if m.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if m.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let final_ch = if release { 'm' } else { 'M' };
    Some(format!("\x1b[<{};{};{}{}", code, m.column + 1, m.row + 1, final_ch).into_bytes())
}

pub fn render_frame(
    f: &mut Frame,
    screen: &vt100::Screen,
    mqtt: Option<&MqttButtonsState>,
    modal: &ModalState,
    agent_state: AgentState,
    border: bool,
) {
    let size = f.area();

    // 默认无边框,终端铺满整屏;所有控制入口在快捷键呼出的菜单弹窗里(见下方)。
    // 菜单 Border 条目(config `[ui] border`)可打开 ratatui 全边框:边框颜色跟随 agent
    // 工作状态,右上角 title 显示 `● <状态>`。PTY 尺寸由 ws 侧随 border 开关同步 resize。
    {
        let (term_rows, term_cols) = screen.size();
        // 先 Clear 整屏:居中的终端只重绘自己的 rect,不清的话其余区域会残留上一帧
        // (更宽时的)旧内容,滚动时尤其明显,看着像重复行。
        f.render_widget(Clear, size);
        let mut content = size;
        if border {
            let color = agent_status_color(agent_state);
            let block = Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(color))
                .title(
                    Line::from(vec![
                        Span::styled("● ", Style::new().fg(color)),
                        Span::raw(agent_status_label(agent_state)),
                    ])
                    .alignment(Alignment::Right),
                );
            content = block.inner(size);
            f.render_widget(block, size);
        }
        // 终端屏幕有固定的 cols×rows,可能小于可用区(如 ESP32 sync 把 PTY resize 成小屏后,
        // 本地窗口仍很大)。按屏幕尺寸在可用区内居中;屏幕大于可用区时铺满并裁切。
        let area = centered_rect(content, term_cols, term_rows);
        let pseudo_term = PseudoTerminal::new(screen);
        f.render_widget(pseudo_term, area);
    }

    // 模态对话框(菜单 / 输入地址 / MQTT 面板 / 错误),覆盖在终端之上。
    if !matches!(modal, ModalState::None) {
        let (w, h) = match modal {
            ModalState::Menu { .. } => (40, 8),
            ModalState::MqttPanel { .. } => (48, 16),
            _ => (48, 9),
        };
        let area = centered_rect(f.area(), w, h);
        f.render_widget(Clear, area);
        let title = match modal {
            ModalState::Menu { .. } => " Menu ",
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
            ModalState::Menu { .. } => "↑↓/Tab move · Enter select · Esc close".to_string(),
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
            ModalState::Menu { focus } => {
                // 5 行:agent 状态(纯显示)+ MQTT / Border / Fit / Quit 四个条目。
                let has_mqtt = mqtt.is_some();
                let rows = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .split(panes[0]);
                // 第一行:OSC 7501 agent 状态(彩色圆点 + 状态名,不可聚焦)。
                let color = agent_status_color(agent_state);
                f.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled("● ", Style::new().fg(color)),
                        Span::raw(agent_status_label(agent_state)),
                    ])),
                    rows[0],
                );
                // 条目行:选中项高亮(LightBlue 底,与 MqttPanel 聚焦行同款)。
                let items: [(MenuFocus, String); 4] = [
                    (
                        MenuFocus::Mqtt,
                        mqtt.map_or_else(|| "MQTT (not configured)".to_string(), mqtt_button_label),
                    ),
                    (
                        MenuFocus::Border,
                        format!("Border: {}", if border { "on" } else { "off" }),
                    ),
                    (MenuFocus::Fit, "Fit".to_string()),
                    (MenuFocus::Quit, "Quit".to_string()),
                ];
                for (i, (item_focus, label)) in items.iter().enumerate() {
                    let row = rows[i + 1];
                    let style = if *focus == *item_focus {
                        Style::default().bg(Color::LightBlue).fg(Color::Black)
                    } else {
                        Style::default()
                    };
                    // 未配 [mqtt]:MQTT 条目置灰(不可聚焦,Enter 也无动作)。
                    let span = if *item_focus == MenuFocus::Mqtt && !has_mqtt {
                        Span::styled(label.as_str(), Style::new().fg(Color::DarkGray))
                    } else {
                        Span::raw(label.as_str())
                    };
                    f.render_widget(Paragraph::new(Line::from(vec![span])).style(style), row);
                }
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

/// footer MQTT 按钮文字:据 broker/client 两个开关组合出简洁状态(render 与点击命中检测共用)。
pub(crate) fn mqtt_button_label(mqtt: &MqttButtonsState) -> String {
    match (mqtt.broker_on, mqtt.client_on) {
        (false, false) => "MQTT off".to_string(),
        (true, false) => "MQTT brkr".to_string(),
        (false, true) => "MQTT conn".to_string(),
        (true, true) => "MQTT on".to_string(),
    }
}

/// 「agent 状态」显示块的 label(Osc 7501 状态名,与 presence 的 state 字串一致)。
fn agent_status_label(state: AgentState) -> &'static str {
    match state {
        AgentState::Working => "working",
        AgentState::Waiting => "waiting",
        AgentState::Idle => "idle",
        AgentState::Done => "done",
        AgentState::Error => "error",
    }
}

/// agent 状态对应的语义色(working=青 / waiting=黄 / idle=灰 / done=绿 / error=红)。
/// 菜单状态行圆点和可选边框(Border 开启时整个边框)共用。
fn agent_status_color(state: AgentState) -> Color {
    match state {
        AgentState::Working => Color::Cyan,
        AgentState::Waiting => Color::Yellow,
        AgentState::Idle => Color::Gray,
        AgentState::Done => Color::Green,
        AgentState::Error => Color::Red,
    }
}

pub fn spawn_event_loop(ui_tx: UITx, menu_key: KeyBinding, mouse_gate: Arc<AtomicBool>) {
    let _thread = std::thread::spawn(move || {
        if let Err(e) = event_loop_thread(ui_tx, menu_key, mouse_gate) {
            log::error!("Event loop error: {}", e);
        }
    });
}

fn event_loop_thread(
    tx_to_pty: UITx,
    menu_key: KeyBinding,
    mouse_gate: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let timeout = Duration::from_millis(500);
    loop {
        if event::poll(timeout)? {
            let evt = event::read()?;

            if let Event::Key(key) = evt {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        // Ctrl+C:原样转发 0x03 给子进程(中断语义与真终端一致),事件循环
                        // **继续跑**。此前这里 return 杀掉事件循环,子进程不退出时(claude 把
                        // Ctrl+C 当回合中断)本地键盘会整体失灵。退出统一走 PTY EOF(子进程
                        // 结束)或菜单 Quit,两条路都会收尾事件循环所在进程。
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = tx_to_pty.blocking_send(UIEvent::Input(Vec::from(&[0x03][..])));
                        }
                        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            let _ = tx_to_pty.blocking_send(UIEvent::Input(Vec::from(&[0x04][..])));
                        }
                        // 菜单快捷键(config `[ui] menu_key`)→ 菜单弹窗开关:拦截,不进 PTY。
                        // 与配置精确比对(修饰键全集一致才算),避免 ctrl+shift+p 误吃
                        // ctrl+alt+shift+p 这类组合。
                        _ if key.code == menu_key.code && key.modifiers == menu_key.modifiers => {
                            log::debug!("[ui] menu key captured, sending ToggleMenu");
                            let _ = tx_to_pty.blocking_send(UIEvent::ToggleMenu);
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
                // 鼠标透传:只有子进程自己请求了鼠标模式(gate)才编码成 SGR 转发,
                // 否则事件直接丢弃(终端侧的上报也已被 ws 侧关掉,这里只是兜底)。
                if mouse_gate.load(Ordering::Relaxed)
                    && let Some(bytes) = encode_mouse_sgr(&mouse)
                {
                    let _ = tx_to_pty.blocking_send(UIEvent::Input(bytes));
                }
            } else if let Event::Resize(cols, rows) = evt {
                let _ = tx_to_pty.blocking_send(UIEvent::Resize(cols, rows));
            }
        }
    }
}

/// 把修饰键编成 xterm 的 modifier code(2..=8),无修饰键返回 None。
/// N = 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0):Shift=2、Alt=3、Alt+Shift=4、
/// Ctrl=5、Ctrl+Shift=6、Ctrl+Alt=7、Ctrl+Alt+Shift=8。
fn modifier_code(mods: KeyModifiers) -> Option<u8> {
    let shift = mods.contains(KeyModifiers::SHIFT);
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    if !(shift || alt || ctrl) {
        return None;
    }
    Some(1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8)
}

/// 拼一个(可能带修饰键的)CSI 导航/功能键序列。`param=None` 表示字母末尾键
/// (方向键/Home/End):裸 `\x1b[A`、带修饰 `\x1b[1;3A`;`param=Some("5")` 表示
/// `~` 末尾键(PgUp/Delete 等):裸 `\x1b[5~`、带修饰 `\x1b[5;3~`。修饰键存在时
/// 按 xterm Modified Keys 规范插入 `;N`,否则发裸序列。
fn push_csi_key(bytes: &mut Vec<u8>, param: Option<&str>, final_ch: char, mods: KeyModifiers) {
    bytes.push(0x1b);
    bytes.push(b'[');
    match modifier_code(mods) {
        None => {
            if let Some(p) = param {
                bytes.extend_from_slice(p.as_bytes());
            }
        }
        Some(n) => {
            // 字母键无 param → 用 1;数字键用原 param。再跟 `;N`(N 是 2..=8 的单数字)。
            bytes.extend_from_slice(param.unwrap_or("1").as_bytes());
            bytes.push(b';');
            bytes.push(b'0' + n);
        }
    }
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(final_ch.encode_utf8(&mut buf).as_bytes());
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
        // Backspace 发 DEL(0x7f),不是 BS(0x08):现代终端/应用(readline、line editor、
        // helix/zerostack 等)都把 Backspace 认作 0x7f;发 0x08 会被当成 Ctrl+H。
        KeyCode::Backspace => bytes.push(0x7f),
        KeyCode::Esc => bytes.push(0x1b),
        // 导航键带修饰键时按 xterm 规范加 `;N` 后缀(Ctrl/Shift/Alt 组合),裸键不变。
        // 例:Ctrl+→ = \x1b[1;5C、Shift+↑ = \x1b[1;2A、Ctrl+Delete = \x1b[3;5~。
        KeyCode::Up => push_csi_key(&mut bytes, None, 'A', key.modifiers),
        KeyCode::Down => push_csi_key(&mut bytes, None, 'B', key.modifiers),
        KeyCode::Left => push_csi_key(&mut bytes, None, 'D', key.modifiers),
        KeyCode::Right => push_csi_key(&mut bytes, None, 'C', key.modifiers),
        KeyCode::Home => push_csi_key(&mut bytes, None, 'H', key.modifiers),
        KeyCode::End => push_csi_key(&mut bytes, None, 'F', key.modifiers),
        KeyCode::PageUp => push_csi_key(&mut bytes, Some("5"), '~', key.modifiers),
        KeyCode::PageDown => push_csi_key(&mut bytes, Some("6"), '~', key.modifiers),
        KeyCode::Delete => push_csi_key(&mut bytes, Some("3"), '~', key.modifiers),
        KeyCode::Insert => push_csi_key(&mut bytes, Some("2"), '~', key.modifiers),
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
