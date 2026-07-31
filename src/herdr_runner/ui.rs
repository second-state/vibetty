//! herdr 模式的极简 TUI:只占 herdr 分出的 1 行 pane,显示一行状态。
//!
//! 不渲染 PTY 内容(agent 在上方自己的 pane 里看);本地按键不转发进 PTY,
//! 只用 `Ctrl+C` / `q` 退出 vibetty。ratatui/crossterm 初始化复用 `crate::ui`。

use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use tokio::sync::mpsc;

/// herdr 本地 TUI 只关心这两件事:退出 / 窗口尺寸变化(重画状态条)。
/// 普通按键和鼠标事件一律丢弃,不转发进 PTY。尺寸值不读(PTY 大小由远端 sync 驱动)。
pub enum HerdrUiEvent {
    Quit,
    Resize,
}

pub type HerdrTerminal = Terminal<CrosstermBackend<io::Stdout>>;

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

/// 画 1 行状态:`vibetty ▸ <target> · MQTT <state>`。占满整个 pane(1 行高,自动截断)。
pub fn draw_status(terminal: &mut HerdrTerminal, target: &str, mqtt_state: &str) -> io::Result<()> {
    terminal.draw(|f| {
        let area: Rect = f.area();
        let line = Line::from(vec![
            Span::raw("vibetty ▸ "),
            Span::styled(target, Style::default()),
            Span::raw(" · MQTT "),
            Span::raw(mqtt_state),
        ]);
        f.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), area);
    })?;
    Ok(())
}
