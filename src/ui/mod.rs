use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
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
use vt100::Callbacks;

struct WindowCallbacks {
    title: String,
    icon_name: String,
    update_title: bool,
}

impl WindowCallbacks {
    fn new() -> Self {
        Self {
            title: String::new(),
            icon_name: String::new(),
            update_title: false,
        }
    }
}

impl Callbacks for WindowCallbacks {
    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, icon_name: &[u8]) {
        self.icon_name = std::str::from_utf8(icon_name).unwrap().to_string();
        self.update_title = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = std::str::from_utf8(title).unwrap().to_string();
        self.update_title = true;
    }
}

pub enum UIEvent {
    Input(Vec<u8>),
    Title(String),
}

pub type UITx = mpsc::Sender<UIEvent>;

pub struct App {
    header_text: String,
    footer_text: String,
    parser: vt100::Parser<WindowCallbacks>,
    rx_from_pty: mpsc::Receiver<Bytes>,
}

impl App {
    pub fn new(
        header_text: String,
        footer_text: String,
        rx_from_pty: mpsc::Receiver<Bytes>,
    ) -> Self {
        Self {
            header_text,
            footer_text,
            parser: vt100::Parser::new_with_callbacks(24, 80, 0, WindowCallbacks::new()),
            rx_from_pty,
        }
    }

    /// 处理来自 PTY 的输出数据（阻塞式）
    fn process_pty_output(&mut self) -> bool {
        match self.rx_from_pty.blocking_recv() {
            Some(bytes) => {
                log::trace!("Received {} bytes from PTY", bytes.len());
                self.parser.process(&bytes);
                // 处理完一个后，非阻塞地检查是否有更多数据
                loop {
                    match self.rx_from_pty.try_recv() {
                        Ok(bytes) => {
                            self.parser.process(&bytes);
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                true
            }
            None => false,
        }
    }

    pub fn run(&mut self, ui_tx: UITx) -> io::Result<()> {
        enable_raw_mode()?;
        let mut stdout = std::io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let term_size = terminal.size()?;
        let header_height = 3u16;
        let footer_height = 3u16;
        let content_rows = term_size
            .height
            .saturating_sub(header_height + footer_height);
        let content_cols = term_size.width.saturating_sub(4);

        // 设置 parser 初始尺寸
        self.parser
            .screen_mut()
            .set_size(content_rows, content_cols);

        // 创建事件循环线程
        let shutdown = Arc::new(AtomicBool::new(false));
        let event_tx = ui_tx.clone();
        let event_shutdown = shutdown.clone();
        let event_thread = std::thread::spawn(move || {
            if let Err(e) = event_loop_thread(event_tx, event_shutdown) {
                log::error!("Event loop error: {}", e);
            }
        });

        loop {
            if !self.process_pty_output() {
                break;
            }

            if event_thread.is_finished() {
                break;
            }

            let callback = self.parser.callbacks_mut();
            if callback.update_title {
                ui_tx
                    .blocking_send(UIEvent::Title(format!("{}", callback.title)))
                    .ok();
                callback.update_title = false;
            }

            terminal.draw(|f| self.ui(f))?;
        }

        log::info!("Shutting down event loop thread...");
        shutdown.store(true, Ordering::Relaxed);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        Ok(())
    }

    fn ui(&mut self, f: &mut Frame) {
        let size = f.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(size);

        let content_rows = chunks[1].height.saturating_sub(2);
        let content_cols = chunks[1].width.saturating_sub(2);

        // Update parser size if changed
        {
            let (current_rows, current_cols) = self.parser.screen().size();
            if current_rows != content_rows || current_cols != content_cols {
                self.parser
                    .screen_mut()
                    .set_size(content_rows, content_cols);
            }
        }

        let title = format!("{}", self.parser.callbacks().title,);

        let header = Paragraph::new(self.header_text.as_str())
            .block(Block::new().borders(Borders::ALL).title(title))
            .alignment(Alignment::Center);
        f.render_widget(header, chunks[0]);

        {
            let pseudo_term = PseudoTerminal::new(self.parser.screen())
                .block(Block::new().borders(Borders::ALL).title("Terminal"));
            f.render_widget(pseudo_term, chunks[1]);
        }

        let footer = Paragraph::new(self.footer_text.as_str())
            .block(Block::new().borders(Borders::ALL).title("Status"))
            .alignment(Alignment::Center);
        f.render_widget(footer, chunks[2]);
    }
}

/// 事件循环线程 - 负责读取键盘事件并发送到 PTY
fn event_loop_thread(tx_to_pty: UITx, shutdown: Arc<AtomicBool>) -> anyhow::Result<()> {
    let timeout = Duration::from_millis(500);
    loop {
        // 检查是否应该退出
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

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
            }
        }
    }

    Ok(())
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
        KeyCode::Tab => bytes.push(b'\t'),
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
