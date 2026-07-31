//! herdr 模式的极简 TUI:只占 herdr 分出的 1 行 pane,显示一行状态。
//!
//! 不渲染 PTY 内容(agent 在上方自己的 pane 里看);本地按键不转发进 PTY,
//! 只用 `Ctrl+C` / `q` 退出 vibetty。**只监听键盘,不开 mouse capture**(不
//! 复用 `crate::ui::init_terminal`,那个会 EnableMouseCapture)。

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use tokio::sync::mpsc;

/// herdr 本地 TUI 只关心这两件事:退出 / 窗口尺寸变化(重画状态条)。
/// 普通按键和鼠标事件一律丢弃,不转发进 PTY。尺寸值不读(PTY 大小由远端 sync 驱动)。
pub enum HerdrUiEvent {
    Quit,
    Resize,
}

pub type HerdrTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// 进入 raw mode + 备用屏。**不**开 mouse capture(herdr 状态条只用键盘)。
pub fn init_terminal() -> io::Result<HerdrTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

/// 还原终端:退出备用屏 + 关 raw mode。
pub fn cleanup_terminal(terminal: &mut HerdrTerminal) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

/// 后台线程:轮询 crossterm 事件,只把 `Ctrl+C` / `q` → Quit、Resize 转发出去。
pub fn spawn_event_loop(tx: mpsc::Sender<HerdrUiEvent>) {
    std::thread::spawn(move || {
        if let Err(e) = event_loop_thread(tx) {
            log::error!("[herdr ui] event loop error: {e}");
        }
    });
}

fn event_loop_thread(tx: mpsc::Sender<HerdrUiEvent>) -> anyhow::Result<()> {
    let timeout = std::time::Duration::from_millis(500);
    loop {
        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    // Ctrl+C 或 q → 退出 vibetty(不转发给 PTY)。
                    KeyCode::Char('c') if ctrl => {
                        let _ = tx.blocking_send(HerdrUiEvent::Quit);
                        return Ok(());
                    }
                    KeyCode::Char('q') if !ctrl => {
                        let _ = tx.blocking_send(HerdrUiEvent::Quit);
                        return Ok(());
                    }
                    _ => {}
                }
            }
            Event::Resize(_cols, _rows) => {
                let _ = tx.blocking_send(HerdrUiEvent::Resize);
            }
            // 其余按键 / 鼠标 / 粘贴:丢弃,不转发。
            _ => {}
        }
    }
}

/// 画 1 行状态:`<agent> ▸ <target> · [MQTT <X.XX MB>] · <title>`。
/// MQTT 连上时整个 `[MQTT ...]` 绿色;括号里的 MB 是出站 screen 字节累计。不换行——
/// 超出 pane 宽度的部分直接截断(避免把 1 行高的 pane 撑成多行)。
pub fn draw_status(
    terminal: &mut HerdrTerminal,
    agent: &str,
    target: &str,
    mqtt_connected: bool,
    mqtt_bytes: u64,
    title: &str,
) -> io::Result<()> {
    terminal.draw(|f| {
        let area: Rect = f.area();
        let mqtt_color = if mqtt_connected {
            Color::Green
        } else {
            Color::DarkGray
        };
        let mqtt_label = format!("[MQTT · {:.2} MB]", mqtt_bytes as f64 / (1024.0 * 1024.0));
        let line = Line::from(vec![
            Span::raw(agent),
            Span::raw(" ▸ "),
            Span::raw(target),
            Span::raw(" · "),
            Span::styled(mqtt_label, Style::default().fg(mqtt_color)),
            Span::raw(" · "),
            Span::raw(title),
        ]);
        f.render_widget(Paragraph::new(line), area);
    })?;
    Ok(())
}
