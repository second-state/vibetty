use portable_pty::{Child, MasterPty, PtySize};
use tokio::sync::{mpsc, oneshot};

pub mod agent;
pub mod pty;

/// A write request handed off to the blocking writer thread: the bytes to
/// write plus a one-shot channel used to acknowledge the result.
pub(crate) type WriteMsg = (Vec<u8>, oneshot::Sender<std::io::Result<()>>);

/// A terminal session backed by a cross-platform PTY.
///
/// `portable-pty` exposes blocking `std::io` handles rather than async ones,
/// so the actual reads and writes happen on dedicated blocking threads and are
/// bridged to async via tokio channels:
///
/// - a reader thread owns the PTY reader and pushes output chunks onto
///   `read_rx`; `read()` returns them verbatim as owned `Vec<u8>`.
/// - a writer thread owns the PTY writer and drains `write_rx`; `write_all()`
///   hands it bytes plus a one-shot ack and awaits the result.
pub struct EchokitChild {
    uuid: uuid::Uuid,
    master: Box<dyn MasterPty + Send>,
    child: Option<Box<dyn Child + Sync + Send>>,
    read_rx: mpsc::Receiver<Vec<u8>>,
    write_tx: mpsc::Sender<WriteMsg>,
}

#[allow(unused)]
impl EchokitChild {
    pub fn session_id(&self) -> uuid::Uuid {
        self.uuid
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.write_tx
            .send((buf.to_vec(), ack_tx))
            .await
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        match ack_rx.await {
            Ok(res) => res,
            Err(_) => Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        }
    }

    pub async fn send_key_iter<S: AsRef<[u8]>>(&mut self, keys: &[S]) -> std::io::Result<()> {
        for key in keys {
            self.write_all(key.as_ref()).await?;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }

    pub async fn send_text(&mut self, text: &str) -> std::io::Result<()> {
        self.write_all(text.as_bytes()).await
    }

    pub async fn send_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_all(bytes).await
    }

    pub async fn send_esc(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x1b").await
    }

    pub async fn send_up_arrow(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x1b[A").await
    }

    pub async fn send_down_arrow(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x1b[B").await
    }

    pub async fn send_left_arrow(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x1b[D").await
    }

    pub async fn send_right_arrow(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x1b[C").await
    }

    pub async fn send_keyboard_interrupt(&mut self) -> std::io::Result<()> {
        self.write_all(b"\x03").await
    }

    pub async fn send_enter(&mut self) -> std::io::Result<()> {
        self.write_all(b"\r").await
    }

    /// Pull the next chunk of PTY output off the channel.
    ///
    /// Returns the chunk verbatim (whatever the reader thread read in one go).
    /// An empty `Vec` signals EOF / channel closed (the child exited or the
    /// reader thread stopped). Because chunks are returned owned and whole,
    /// there is no caller buffer to underfill — no partial-read buffering.
    pub async fn read(&mut self) -> std::io::Result<Vec<u8>> {
        Ok(self.read_rx.recv().await.unwrap_or_default())
    }

    pub async fn read_string(&mut self) -> std::io::Result<String> {
        let mut string_buffer = Vec::with_capacity(512);

        loop {
            let chunk = self.read().await?;
            if chunk.is_empty() {
                break;
            }

            string_buffer.extend_from_slice(&chunk);

            if str::from_utf8(&string_buffer).is_ok() {
                return Ok(String::from_utf8_lossy(&string_buffer).into_owned());
            }
        }

        Ok(String::from_utf8_lossy(&string_buffer).into_owned())
    }

    pub async fn wait(&mut self) -> anyhow::Result<portable_pty::ExitStatus> {
        let mut child = self
            .child
            .take()
            .ok_or_else(|| anyhow::anyhow!("child process handle already consumed"))?;
        Ok(tokio::task::spawn_blocking(move || child.wait()).await??)
    }

    pub async fn kill(&mut self) -> anyhow::Result<()> {
        let child = self
            .child
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("child process handle already consumed"))?;
        let mut killer = child.clone_killer();
        tokio::task::spawn_blocking(move || killer.kill()).await?;
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(std::io::Error::other)
    }

    pub async fn read_pty_output(&mut self) -> std::io::Result<String> {
        let mut string_buffer = Vec::with_capacity(512);

        let chunk = self.read().await?;
        if chunk.is_empty() {
            return Ok(String::new());
        }
        string_buffer.extend_from_slice(&chunk);

        // Drain more chunks until we have a complete UTF-8 sequence
        // (a multi-byte char may be split across chunks).
        loop {
            if str::from_utf8(&string_buffer).is_ok() {
                break;
            }

            let chunk = self.read().await?;
            if chunk.is_empty() {
                break;
            }
            string_buffer.extend_from_slice(&chunk);
        }

        Ok(String::from_utf8_lossy(&string_buffer).into_owned())
    }
}
