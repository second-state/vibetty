use std::collections::LinkedList;

use linemux::Line;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::types::claude::ClaudeCodeLog;

use super::{EchokitChild, PtyCommand, PtySize, TerminalType};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UseTool {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state")]
pub enum ClaudeCodeState {
    PreUseTool {
        request: Vec<UseTool>,
        is_pending: bool,
        #[serde(skip)]
        start_time: std::time::Instant,
    },
    Output {
        output: String,
        is_thinking: bool,
    },
    StopUseTool {
        is_error: bool,
    },
    Idle,
    Working {
        prompt: String,
    },
}

#[allow(unused)]
impl ClaudeCodeState {
    pub fn input_available(&self) -> bool {
        matches!(
            self,
            ClaudeCodeState::Idle
                | ClaudeCodeState::StopUseTool { .. }
                | ClaudeCodeState::Output {
                    is_thinking: false,
                    ..
                }
        )
    }

    pub fn cancel_available(&self) -> bool {
        matches!(
            self,
            ClaudeCodeState::PreUseTool { .. }
                | ClaudeCodeState::Output {
                    is_thinking: true,
                    ..
                }
        )
    }

    pub fn confirm_available(&self) -> bool {
        self.input_available()
            || matches!(
                self,
                ClaudeCodeState::PreUseTool {
                    is_pending: true,
                    ..
                }
            )
    }

    pub fn is_use_tool(&self) -> bool {
        matches!(self, ClaudeCodeState::PreUseTool { .. })
    }
}

impl std::fmt::Display for ClaudeCodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaudeCodeState::PreUseTool { .. } => write!(f, "pre_use_tool"),
            ClaudeCodeState::Output { is_thinking, .. } => {
                if *is_thinking {
                    write!(f, "thinking")
                } else {
                    write!(f, "output")
                }
            }
            ClaudeCodeState::StopUseTool { is_error } => {
                if *is_error {
                    write!(f, "stop_use_tool_error")
                } else {
                    write!(f, "stop_use_tool")
                }
            }
            ClaudeCodeState::Idle => write!(f, "idle"),
            ClaudeCodeState::Working { .. } => write!(f, "working"),
        }
    }
}

pub struct ClaudeCode {
    history_file: linemux::MuxedLines,
    history_file_path: std::path::PathBuf,
    start_output_buffer: LinkedList<String>,
    state: ClaudeCodeState,
    working: bool,
}

impl TerminalType for ClaudeCode {
    type Output = ClaudeCodeResult;
}

pub async fn new_with_command<S: AsRef<str>>(
    claude_start_shell: &str,
    args: &[S],
    env: &[(S, S)],
    size: (u16, u16),
    current_dir: Option<std::path::PathBuf>,
) -> pty_process::Result<EchokitChild<ClaudeCode>> {
    let (row, col) = size;

    let (mut pty, pts) = pty_process::open()?;
    pty.resize(PtySize::new(row, col))?;

    let mut cmd = PtyCommand::new(claude_start_shell);

    cmd = cmd
        .args(args.iter().map(|arg| arg.as_ref()))
        .env("TERM", "xterm-256color")
        .env("COLUMNS", col.to_string())
        .env("LINES", row.to_string())
        .env("FORCE_COLOR", "1")
        .env("COLORTERM", "truecolor")
        .env("PYTHONUNBUFFERED", "1");

    for (key, value) in env {
        cmd = cmd.env(key.as_ref(), value.as_ref());
    }

    if let Some(current_dir) = current_dir {
        cmd = cmd.current_dir(current_dir);
    }

    let child = cmd.spawn(pts)?;
    log::debug!(
        "Started claude terminal with PID {}",
        child.id().unwrap_or(0)
    );

    let mut start_output_buffer = LinkedList::new();
    let mut buffer = [0u8; 1024];

    loop {
        let n = pty.read(&mut buffer).await?;
        let output = str::from_utf8(&buffer[..n]).unwrap_or("");
        log::trace!("PTY Output during history file check: {}", output);

        start_output_buffer.push_back(output.to_string());

        if output.contains("Claude Code") {
            log::debug!("Claude Code terminal is ready.");
            break;
        }

        if output.contains("Enter to confirm · Esc to cancel") {
            pty.write_all(b"\r").await?;
        }
    }

    log::debug!("Sending /status command to extract session ID and current directory");
    pty.write_all(b"/status").await?;
    pty.flush().await?;

    // get session-id from status
    let mut status_output = String::new();

    loop {
        let n =
            tokio::time::timeout(std::time::Duration::from_secs(1), pty.read(&mut buffer)).await;
        if n.is_err() {
            pty.write_all(b"\r").await?;
            continue;
        }

        let n = n.unwrap()?;

        log::trace!("Read {} bytes from PTY for status output", n);
        let output = str::from_utf8(&buffer[..n]).unwrap_or("");
        log::trace!("PTY Output during history file check: {}", output);

        status_output.push_str(output);
        start_output_buffer.push_back(output.to_string());

        if output.contains("Model:") {
            pty.write_all(b"\x1b").await?;
            break;
        }
    }

    let mut uuid = uuid::Uuid::nil();
    let mut cwd = String::new();
    let status_output = strip_ansi_escapes::strip_str(&status_output);
    for line in status_output.lines() {
        if let Some(session_id) = line.trim().strip_prefix("Session ID:") {
            let session_id = session_id.trim();
            log::debug!("Extracted session ID from status output: {}", session_id);
            if let Ok(uuid_) = uuid::Uuid::parse_str(session_id) {
                uuid = uuid_;
                log::debug!("Parsed session ID as UUID: {}", uuid);
            }
            continue;
        }

        if let Some(session_id) = line.trim().strip_prefix("SessionID:") {
            let session_id = session_id.trim();
            log::debug!("Extracted session ID from status output: {}", session_id);
            if let Ok(uuid_) = uuid::Uuid::parse_str(session_id) {
                uuid = uuid_;
                log::debug!("Parsed session ID as UUID: {}", uuid);
            }
            continue;
        }

        if let Some(cwd_) = line.trim().strip_prefix("cwd:") {
            cwd = cwd_.trim().to_string();
            log::debug!("Extracted current directory from status output: {}", cwd);
            continue;
        }
    }

    if uuid.is_nil() {
        return Err(pty_process::Error::Io(std::io::Error::other(
            "Failed to extract session ID from status output".to_string(),
        )));
    }

    if cwd.is_empty() {
        return Err(pty_process::Error::Io(std::io::Error::other(
            "Failed to extract current directory from status output".to_string(),
        )));
    }

    log::debug!(
        "Final extracted session ID: {}, current directory: {}",
        uuid,
        cwd
    );

    let home_dir = std::env::home_dir().expect("Failed to get home directory");
    let history_file_path = home_dir
        .join(".claude")
        .join("projects")
        .join(cwd.replace(['/', '_'], "-"))
        .join(format!("{}.jsonl", uuid));

    if !history_file_path.exists() {
        std::fs::create_dir_all(history_file_path.parent().unwrap())?;
        std::fs::File::create(&history_file_path)?;
    }

    let mut history_file = linemux::MuxedLines::new().expect("Failed to create MuxedLines");
    log::info!(
        "Storing claude code history in {}",
        history_file_path.display()
    );

    history_file
        .add_file(&history_file_path)
        .await
        .map_err(|e| {
            log::error!("Failed to open claude code history file: {}", e);
            pty_process::Error::Io(e)
        })?;

    Ok(EchokitChild::<ClaudeCode> {
        uuid,
        pty,
        child,
        terminal_type: ClaudeCode {
            history_file,
            history_file_path,
            start_output_buffer,
            state: ClaudeCodeState::Idle,
            working: false,
        },
    })
}

#[allow(clippy::large_enum_variant)]
pub enum ClaudeCodeResult {
    PtyOutput(String),
    ClaudeLog(ClaudeCodeLog),
    WaitForUserInputBeforeTool,
    WaitForUserInput,
    Working,
    Uncaught(String),
}

impl EchokitChild<ClaudeCode> {
    pub fn session_id(&self) -> uuid::Uuid {
        self.uuid
    }

    #[allow(unused)]
    pub fn log_file_path(&self) -> &std::path::PathBuf {
        &self.terminal_type.history_file_path
    }

    pub fn state(&self) -> &ClaudeCodeState {
        &self.terminal_type.state
    }

    pub fn update_title(&mut self, title: String) -> Option<ClaudeCodeResult> {
        match &self.terminal_type.state {
            ClaudeCodeState::PreUseTool { .. } => {
                if title.contains("✳ Claude Code") {
                    self.terminal_type.working = false;
                    Some(ClaudeCodeResult::WaitForUserInputBeforeTool)
                } else {
                    self.terminal_type.working = true;
                    Some(ClaudeCodeResult::Working)
                }
            }
            _ => {
                log::debug!(
                    "Received title update in state {:?}, setting working state",
                    self.terminal_type.state
                );

                self.terminal_type.working = !title.contains("✳ Claude Code");

                None
            }
        }
    }

    pub fn update_state(&mut self, result: &ClaudeCodeResult) -> bool {
        let mut state_updated = false;
        match (result, &mut self.terminal_type.state) {
            (ClaudeCodeResult::PtyOutput(..), _) => {
                log::debug!("Updating state from Idle to Processing");
            }
            (ClaudeCodeResult::Working, _) => {
                log::debug!("Updating state to Working");
                self.terminal_type.state = ClaudeCodeState::Working {
                    prompt: "Continuing...".to_string(),
                };
                state_updated = true;
            }
            (
                ClaudeCodeResult::WaitForUserInputBeforeTool,
                ClaudeCodeState::PreUseTool { is_pending, .. },
            ) => {
                *is_pending = true;
                state_updated = true;
            }
            (ClaudeCodeResult::WaitForUserInput, ClaudeCodeState::Output { .. }) => {
                self.terminal_type.state = ClaudeCodeState::Idle;
                state_updated = true;
            }
            (ClaudeCodeResult::ClaudeLog(log), ClaudeCodeState::PreUseTool { request, .. }) => {
                log::debug!("Processing ClaudeLog in PreUseTool state: {:?}", log);
                let (id, is_error) = log.is_tool_result();

                if !id.is_empty() {
                    if is_error {
                        self.terminal_type.state = ClaudeCodeState::StopUseTool { is_error: true };
                    } else {
                        let len = request.len();
                        for (i, tool) in request.iter_mut().enumerate() {
                            if tool.id == id {
                                tool.done = true;
                                if i == len - 1 {
                                    self.terminal_type.state =
                                        ClaudeCodeState::StopUseTool { is_error: false };
                                }
                                break;
                            }
                        }
                    }
                    state_updated = true;
                    return state_updated;
                }

                if log.is_stop() {
                    self.terminal_type.state = ClaudeCodeState::StopUseTool { is_error: false };
                    state_updated = true;
                } else if let Some((id, name, input)) = log.is_tool_request() {
                    request.push(UseTool {
                        id,
                        name,
                        input,
                        done: false,
                    });
                    state_updated = true;
                }
            }
            (ClaudeCodeResult::ClaudeLog(log), ClaudeCodeState::Working { .. }) => {
                if log.is_stop() {
                    self.terminal_type.state = ClaudeCodeState::Idle;
                    state_updated = true;
                } else if let Some((id, name, input)) = log.is_tool_request() {
                    self.terminal_type.state = ClaudeCodeState::PreUseTool {
                        request: vec![UseTool {
                            id,
                            name,
                            input,
                            done: false,
                        }],
                        is_pending: false,
                        start_time: std::time::Instant::now(),
                    };
                    state_updated = true;
                } else if let Some((output, is_thinking)) = log.is_output() {
                    self.terminal_type.state = ClaudeCodeState::Output {
                        output,
                        is_thinking,
                    };
                    state_updated = true;
                }
            }
            (
                ClaudeCodeResult::ClaudeLog(log),
                ClaudeCodeState::Output {
                    output,
                    is_thinking,
                },
            ) => {
                if let Some(prompt) = log.is_user_prompt() {
                    self.terminal_type.state = ClaudeCodeState::Working { prompt };
                    state_updated = true;
                    return state_updated;
                }
                if log.is_stop() {
                    self.terminal_type.state = ClaudeCodeState::Idle;
                    state_updated = true;
                } else if let Some((id, name, input)) = log.is_tool_request() {
                    self.terminal_type.state = ClaudeCodeState::PreUseTool {
                        request: vec![UseTool {
                            id,
                            name,
                            input,
                            done: false,
                        }],
                        is_pending: false,
                        start_time: std::time::Instant::now(),
                    };
                    state_updated = true;
                } else if let Some((output_, thinking_)) = log.is_output() {
                    *output = output_;
                    *is_thinking = thinking_;
                    state_updated = true;
                }
            }
            (
                ClaudeCodeResult::ClaudeLog(log),
                ClaudeCodeState::Idle | ClaudeCodeState::StopUseTool { .. },
            ) => {
                if let Some(prompt) = log.is_user_prompt() {
                    self.terminal_type.state = ClaudeCodeState::Working { prompt };
                    state_updated = true;
                    return state_updated;
                }

                if log.is_stop() {
                    state_updated = self.terminal_type.state != ClaudeCodeState::Idle;

                    self.terminal_type.state = ClaudeCodeState::Idle;
                } else if let Some((id, name, input)) = log.is_tool_request() {
                    self.terminal_type.state = ClaudeCodeState::PreUseTool {
                        request: vec![UseTool {
                            id,
                            name,
                            input,
                            done: false,
                        }],
                        is_pending: false,
                        start_time: std::time::Instant::now(),
                    };
                    state_updated = true;
                } else if let Some((output, is_thinking)) = log.is_output() {
                    self.terminal_type.state = ClaudeCodeState::Output {
                        output,
                        is_thinking,
                    };
                    state_updated = true;
                }
            }
            (ClaudeCodeResult::WaitForUserInputBeforeTool, state) => {
                log::debug!(
                    "Received WaitForUserInputBeforeTool in state {:?}, no state change",
                    state
                );
            }
            (ClaudeCodeResult::WaitForUserInput, state) => {
                log::debug!(
                    "Received WaitForUserInput in state {:?}, no state change",
                    state
                );
            }
            (ClaudeCodeResult::Uncaught(s), _) => {
                log::debug!("Uncaught output from ClaudeCode terminal: {}", s);
            }
        }

        state_updated
    }

    pub async fn read_pty_output_and_history_line(&mut self) -> std::io::Result<ClaudeCodeResult> {
        if let Some(pty_output) = self.terminal_type.start_output_buffer.pop_front() {
            log::trace!("Returning buffered PTY output: {}", pty_output);
            return Ok(ClaudeCodeResult::PtyOutput(pty_output));
        }

        let mut buffer = [0u8; 1024];
        let mut string_buffer = Vec::with_capacity(512);

        #[derive(Debug)]
        enum SelectResult {
            Line(Option<Line>),
            Pty(usize),
        }

        let state = &mut self.terminal_type.state;
        let claude_is_working = self.terminal_type.working;

        let read_buff = async {
            match state {
                ClaudeCodeState::PreUseTool { is_pending, .. } => {
                    if !*is_pending && !claude_is_working {
                        log::debug!(
                            "PreUseTool state, waiting for user input before tool, setting read timeout to 5 seconds"
                        );
                        return Err(ClaudeCodeResult::WaitForUserInputBeforeTool);
                    }

                    Ok(self.pty.read(&mut buffer).await)
                }

                ClaudeCodeState::Output {
                    is_thinking: false, ..
                }
                | ClaudeCodeState::StopUseTool { is_error: true } => {
                    if !claude_is_working {
                        Err(ClaudeCodeResult::WaitForUserInput)
                    } else {
                        Ok(self.pty.read(&mut buffer).await)
                    }
                }
                _ => Ok(self.pty.read(&mut buffer).await),
            }
        };

        let r = tokio::select! {
            n = read_buff => {
                match n {
                    Err(timeout) => return Ok(timeout),
                    Ok(n) =>  SelectResult::Pty(n?)
                }
            }
            line = self.terminal_type.history_file.next_line() => {
                SelectResult::Line(line?)
            }
        };

        log::trace!("Select result: {:?}", r);

        match r {
            SelectResult::Line(line_opt) => {
                return if let Some(line) = line_opt {
                    let cc_log = serde_json::from_str::<ClaudeCodeLog>(line.line());

                    if let Ok(r) = cc_log {
                        Ok(ClaudeCodeResult::ClaudeLog(r))
                    } else {
                        Ok(ClaudeCodeResult::Uncaught(line.line().to_string()))
                    }
                } else {
                    Ok(ClaudeCodeResult::Uncaught(String::new()))
                };
            }
            SelectResult::Pty(n) => {
                if n == 0 {
                    return Ok(ClaudeCodeResult::PtyOutput(String::new()));
                }

                string_buffer.extend_from_slice(&buffer[..n]);
            }
        }

        loop {
            let s = str::from_utf8(&string_buffer);
            if let Ok(s) = s {
                return Ok(ClaudeCodeResult::PtyOutput(s.to_string()));
            }

            let n = self.pty.read(&mut buffer).await?;
            log::trace!("Read {} bytes from PTY", n);
            if n == 0 {
                break;
            }

            string_buffer.extend_from_slice(&buffer[..n]);
        }

        Ok(ClaudeCodeResult::PtyOutput(
            String::from_utf8_lossy(&string_buffer).to_string(),
        ))
    }
}

#[tokio::test]
async fn test_linemux() {
    let mut linemux = linemux::MuxedLines::new().unwrap();
    linemux
        .add_file("README.md")
        .await
        .expect("Failed to add file");

    for _ in 0..5 {
        if let Some(line) = linemux.next_line().await.unwrap() {
            println!("Line: {}", line.line());
        } else {
            println!("No more lines");
            break;
        }
    }
}
