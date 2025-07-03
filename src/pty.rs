use crate::error::{Result, RwShellError};
use async_trait::async_trait;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use tokio::sync::broadcast;
use tracing::info;

#[async_trait]
pub trait PtyHandler: Send {
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn refresh(&mut self) -> Result<()>;
}

pub struct PtyMaster {
    child: Option<Box<dyn Child + Send>>,
    size_tx: Option<broadcast::Sender<(u16, u16)>>,
    headless: bool,
    cols: u16,
    rows: u16,
}

impl PtyMaster {
    pub fn new(headless: bool, cols: u16, rows: u16) -> Self {
        Self {
            child: None,
            size_tx: None,
            headless,
            cols,
            rows,
        }
    }

    pub async fn start(&mut self) -> Result<()> {
        let pty_system = native_pty_system();
        let pty_pair = pty_system
            .openpty(PtySize {
                rows: self.rows,
                cols: self.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RwShellError::Pty(format!("Failed to create PTY pair: {e:?}")))?;

        let mut cmd = CommandBuilder::new("/bin/bash");

        if self.headless {
            cmd.env("TERM", "xterm-256color");
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| RwShellError::Pty(format!("Failed to spawn command: {e:?}")))?;

        self.child = Some(child);

        info!("PTY started successfully");
        Ok(())
    }

    pub fn create_size_broadcaster(&mut self) -> broadcast::Receiver<(u16, u16)> {
        let (tx, rx) = broadcast::channel(16);
        self.size_tx = Some(tx);
        rx
    }
}

#[async_trait]
impl PtyHandler for PtyMaster {
    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        // For simplicity, we'll just return the data length
        // In a real implementation, you would write to the PTY master
        Ok(data.len())
    }

    async fn refresh(&mut self) -> Result<()> {
        // Send clear screen sequence
        self.write(&[0x0C]).await?;
        Ok(())
    }
}
