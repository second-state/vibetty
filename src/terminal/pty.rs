use std::io::{Read, Write};

use super::{EchokitChild, WriteMsg};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

pub async fn new_with_command<S: AsRef<str>>(
    shell: &str,
    args: &[S],
    env: &[(S, S)],
    size: (u16, u16),
) -> anyhow::Result<EchokitChild> {
    let (row, col) = size;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: row,
        cols: col,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new(shell);
    for arg in args.iter() {
        cmd.arg(arg.as_ref());
    }
    // Explicitly inherit vibetty's environment instead of relying on
    // portable-pty's implicit snapshot taken inside CommandBuilder::new
    // (get_base_env). portable-pty already captures std::env::vars_os there,
    // but — like its cwd default — that's implicit behavior we'd rather not
    // depend on, so we set it ourselves.
    for (key, value) in std::env::vars_os() {
        cmd.env(key, value);
    }

    cmd.env("TERM", "xterm-256color");
    cmd.env("COLUMNS", col.to_string());
    cmd.env("LINES", row.to_string());
    cmd.env("FORCE_COLOR", "1");
    cmd.env("COLORTERM", "truecolor");
    // cmd.env("PYTHONUNBUFFERED", "1");

    for (key, value) in env {
        cmd.env(key.as_ref(), value.as_ref());
    }

    // portable-pty defaults the working directory to $HOME when cwd is unset
    // (see CommandBuilder::as_command), whereas pty-process inherited the
    // parent's cwd. Fall back to vibetty's own cwd so the spawned program
    // launches in the directory the user started vibetty from.
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let child = pair.slave.spawn_command(cmd)?;
    log::debug!("Started terminal with PID {:?}", child.process_id());

    // The parent no longer needs the slave handle; the child holds its own.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let (read_tx, read_rx) = mpsc::channel::<Vec<u8>>(128);
    let (write_tx, mut write_rx) = mpsc::channel::<WriteMsg>(128);
    // A second handle on the write channel so the reader thread can answer
    // ConPTY's startup cursor-position report on Windows (see below).
    let query_tx = write_tx.clone();

    // Reader thread: owns the blocking PTY reader, pushes output chunks onto a
    // channel consumed by the async `read()` path.
    //
    // It answers exactly one terminal query: the cursor-position report
    // (`ESC[6n`) that Windows ConPTY emits in its first output chunk at startup.
    // ConPTY blocks until it gets a reply, and since vibetty is only a screen
    // emulator it would otherwise never answer — stalling the Windows PTY. Unix
    // PTYs / shells never send it, so on macOS/Linux this branch is a no-op.
    // Capability probes (DA1/DA2/DSR) from TUI apps are intentionally left
    // unanswered — empirically (e.g. codex) apps render correctly without them.
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; 8192];
        let mut first = true; // only inspect the first chunk for ConPTY's ESC[6n
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let chunk = &buf[..n];
                    if first {
                        if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                            // Reply with cursor position row 1, col 1.
                            let (ack_tx, _ack_rx) = oneshot::channel();
                            let _ = query_tx.blocking_send((b"\x1b[1;1R".to_vec(), ack_tx));
                        }
                        first = false;
                    }
                    if read_tx.blocking_send(chunk.to_vec()).is_err() {
                        break; // receiver dropped -> stop pumping
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Writer thread: drains input chunks from a channel and writes them to the
    // PTY, acknowledging each request via a one-shot channel.
    let mut writer = pair.master.take_writer()?;
    tokio::task::spawn_blocking(move || {
        while let Some((buf, ack)) = write_rx.blocking_recv() {
            let res = writer.write_all(&buf).and_then(|_| writer.flush());
            let _ = ack.send(res);
        }
    });

    Ok(EchokitChild {
        uuid: uuid::Uuid::new_v4(),
        master: pair.master,
        child: Some(child),
        read_rx,
        write_tx,
    })
}
