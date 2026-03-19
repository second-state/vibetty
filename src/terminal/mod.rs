use pty_process::{Command, Pty, Size};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Child,
};

pub mod claude;

pub type PtyCommand = Command;
pub type PtySize = Size;

pub trait TerminalType {
    type Output;
}

pub struct EchokitChild<T: TerminalType> {
    uuid: uuid::Uuid,
    pty: Pty,
    child: Child,
    terminal_type: T,
}

#[allow(unused)]
impl<T: TerminalType> EchokitChild<T> {
    pub async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.pty.write_all(buf).await?;
        self.pty.flush().await
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

    pub async fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.pty.read(buffer).await
    }

    pub async fn read_string(&mut self) -> std::io::Result<String> {
        let mut buffer = [0u8; 1024];
        let mut string_buffer = Vec::with_capacity(512);

        loop {
            let n = self.pty.read(&mut buffer).await?;
            if n == 0 {
                break;
            }

            string_buffer.extend_from_slice(&buffer[..n]);

            let s = str::from_utf8(&string_buffer);
            if let Ok(s) = s {
                return Ok(s.to_string());
            }
        }

        Ok(String::from_utf8_lossy(&string_buffer).to_string())
    }

    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    pub async fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill().await
    }
}
