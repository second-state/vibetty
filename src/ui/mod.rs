use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tui_term::widget::PseudoTerminal;

pub enum UIEvent {
    Input(Vec<u8>),
    ScrollUp,
    ScrollDown,
    Resize(u16, u16),
    ResizePtyWidth(i16),
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
    footer_text: &str,
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
        let pseudo_term = PseudoTerminal::new(screen).block(Block::new().borders(Borders::ALL));
        f.render_widget(pseudo_term, chunks[1]);
    }

    let footer = Paragraph::new(footer_text)
        .block(Block::new().borders(Borders::ALL))
        .alignment(Alignment::Center);
    f.render_widget(footer, chunks[2]);
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
                        KeyCode::Char('+') if key.modifiers.contains(KeyModifiers::ALT) => {
                            log::debug!("ALT + '+' detected, sending ResizePtyWidth(5)");
                            let _ = tx_to_pty.blocking_send(UIEvent::ResizePtyWidth(5));
                        }
                        KeyCode::Char('=') if key.modifiers.contains(KeyModifiers::ALT) => {
                            log::debug!("ALT + '=' detected, sending ResizePtyWidth(5)");
                            let _ = tx_to_pty.blocking_send(UIEvent::ResizePtyWidth(5));
                        }
                        KeyCode::Char('-') if key.modifiers.contains(KeyModifiers::ALT) => {
                            log::debug!("ALT + '-' detected, sending ResizePtyWidth(-5)");
                            let _ = tx_to_pty.blocking_send(UIEvent::ResizePtyWidth(-5));
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
                        let _ = tx_to_pty.blocking_send(UIEvent::ScrollUp);
                    }
                    MouseEventKind::ScrollDown => {
                        let _ = tx_to_pty.blocking_send(UIEvent::ScrollDown);
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
