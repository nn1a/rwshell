use crate::error::{Result, RwShellError};
use async_trait::async_trait;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtyPair, PtySize};
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info};

#[async_trait]
pub trait PtyHandler: Send + Sync {
    async fn write(&mut self, data: &[u8]) -> Result<usize>;
    async fn refresh(&mut self) -> Result<()>;
}

pub struct PtyMaster {
    pty_pair: Option<PtyPair>,
    child: Option<Box<dyn Child + Send + Sync>>,
    master: Option<Box<dyn MasterPty + Send + Sync>>,
    size_tx: Option<broadcast::Sender<(u16, u16)>>,
    headless: bool,
    cols: u16,
    rows: u16,
}

impl PtyMaster {
    pub fn new(headless: bool, cols: u16, rows: u16) -> Self {
        Self {
            pty_pair: None,
            child: None,
            master: None,
            size_tx: None,
            headless,
            cols,
            rows,
        }
    }

    pub async fn start(&mut self, command: &str, args: &[String], env_vars: &[String]) -> Result<()> {
        let pty_system = native_pty_system();
        
        // Get initial size
        let (cols, rows) = if self.headless {
            (self.cols, self.rows)
        } else {
            get_terminal_size().unwrap_or((self.cols, self.rows))
        };

        let pty_pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| RwShellError::Pty(format!("Failed to create PTY: {}", e)))?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        
        // Set environment variables
        for env_var in env_vars {
            if let Some((key, value)) = env_var.split_once('=') {
                cmd.env(key, value);
            }
        }

        let child = pty_pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| RwShellError::Pty(format!("Failed to spawn command: {}", e)))?;

        self.pty_pair = Some(pty_pair);
        self.child = Some(child);
        
        // Set up window size change notifications
        let (size_tx, _) = broadcast::channel(16);
        self.size_tx = Some(size_tx);

        info!("PTY started with command: {} {:?}", command, args);
        Ok(())
    }

    pub fn get_master(&mut self) -> Option<&mut Box<dyn MasterPty + Send + Sync>> {
        if let Some(ref mut pty_pair) = self.pty_pair {
            self.master = Some(pty_pair.master.take());
        }
        self.master.as_mut()
    }

    pub async fn set_win_size(&mut self, rows: u16, cols: u16) -> Result<()> {
        if let Some(ref mut pty_pair) = self.pty_pair {
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            
            pty_pair
                .master
                .resize(size)
                .map_err(|e| RwShellError::Pty(format!("Failed to resize PTY: {}", e)))?;
                
            // Notify subscribers of size change
            if let Some(ref size_tx) = self.size_tx {
                let _ = size_tx.send((cols, rows));
            }
        }
        Ok(())
    }

    pub fn get_win_size(&self) -> (u16, u16) {
        if self.headless {
            (self.cols, self.rows)
        } else {
            get_terminal_size().unwrap_or((self.cols, self.rows))
        }
    }

    pub fn subscribe_size_changes(&self) -> Option<broadcast::Receiver<(u16, u16)>> {
        self.size_tx.as_ref().map(|tx| tx.subscribe())
    }

    pub async fn wait(&mut self) -> Result<()> {
        if let Some(ref mut child) = self.child {
            let status = child
                .wait()
                .map_err(|e| RwShellError::Pty(format!("Failed to wait for child: {}", e)))?;
            info!("Child process exited with status: {:?}", status);
        }
        Ok(())
    }

    pub fn make_raw(&self) -> Result<()> {
        // For headless mode, we don't need to make the terminal raw
        if self.headless {
            return Ok(());
        }

        // On Unix systems, we need to set the terminal to raw mode
        #[cfg(unix)]
        {
            use libc::{tcgetattr, tcsetattr, termios, STDIN_FILENO, TCSANOW};
            use std::mem;

            unsafe {
                let mut termios: termios = mem::zeroed();
                if tcgetattr(STDIN_FILENO, &mut termios) != 0 {
                    return Err(RwShellError::Pty("Failed to get terminal attributes".to_string()));
                }

                // Save original settings (we should restore them later)
                // Make the terminal raw
                libc::cfmakeraw(&mut termios);
                
                if tcsetattr(STDIN_FILENO, TCSANOW, &termios) != 0 {
                    return Err(RwShellError::Pty("Failed to set terminal attributes".to_string()));
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl PtyHandler for PtyMaster {
    async fn write(&mut self, data: &[u8]) -> Result<usize> {
        if let Some(ref mut pty_pair) = self.pty_pair {
            pty_pair
                .master
                .write(data)
                .map_err(|e| RwShellError::Pty(format!("Failed to write to PTY: {}", e)))
        } else {
            Err(RwShellError::Pty("PTY not initialized".to_string()))
        }
    }

    async fn refresh(&mut self) -> Result<()> {
        // Send a refresh sequence (Ctrl+L)
        self.write(&[0x0C]).await?;
        Ok(())
    }
}

// Read-only PTY handler that discards writes
pub struct NilPty;

#[async_trait]
impl PtyHandler for NilPty {
    async fn write(&mut self, _data: &[u8]) -> Result<usize> {
        Ok(_data.len()) // Pretend we wrote the data
    }

    async fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
}

fn get_terminal_size() -> Option<(u16, u16)> {
    terminal_size::terminal_size().map(|(width, height)| (width.0, height.0))
}
