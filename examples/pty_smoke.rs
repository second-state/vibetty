//! Minimal portable-pty smoke test, isolated from vibetty's TUI/WS stack.
//!
//! Spawns a shell, waits for its banner, writes `echo hello-pty`, and prints
//! everything read back. Run with:
//!   cargo run --example pty_smoke
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::time::Duration;

fn main() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let prog = if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "bash"
    };
    let builder = CommandBuilder::new(prog);
    let mut child = pair.slave.spawn_command(builder).expect("spawn");
    drop(pair.slave);

    // Reader thread: print every chunk we get back.
    let mut reader = pair.master.try_clone_reader().expect("reader");
    let reader_handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    eprintln!("[smoke] reader: EOF");
                    break;
                }
                Ok(n) => eprintln!(
                    "[smoke] reader: {} bytes: {:?}",
                    n,
                    String::from_utf8_lossy(&buf[..n])
                ),
                Err(e) => {
                    eprintln!("[smoke] reader: error {e}");
                    break;
                }
            }
        }
    });

    // Give the shell a moment to start and emit its banner.
    std::thread::sleep(Duration::from_millis(1500));

    // Send a command we can recognize in the output.
    let cmd_text = if cfg!(target_os = "windows") {
        "echo hello-pty\r\n"
    } else {
        "echo hello-pty\n"
    };
    {
        let mut writer = pair.master.take_writer().expect("writer");
        writer.write_all(cmd_text.as_bytes()).expect("write");
        writer.flush().expect("flush");
        eprintln!("[smoke] wrote {:?}", cmd_text);
    }

    // Let the shell produce output.
    std::thread::sleep(Duration::from_secs(2));

    let _ = child.kill();
    let _ = reader_handle.join();
    eprintln!("[smoke] done");
}
