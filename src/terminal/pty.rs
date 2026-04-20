use super::{EchokitChild, PtyCommand, PtySize};

pub async fn new_with_command<S: AsRef<str>>(
    shell: &str,
    args: &[S],
    env: &[(S, S)],
    size: (u16, u16),
    current_dir: Option<std::path::PathBuf>,
) -> pty_process::Result<EchokitChild> {
    let (row, col) = size;

    let (pty, pts) = pty_process::open()?;
    pty.resize(PtySize::new(row, col))?;

    let mut cmd = PtyCommand::new(shell);

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
    log::debug!("Started terminal with PID {}", child.id().unwrap_or(0));

    Ok(EchokitChild {
        uuid: uuid::Uuid::new_v4(),
        pty,
        child,
    })
}
