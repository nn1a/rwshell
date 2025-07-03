use crate::error::Result;
use crate::pty::PtyHandler;
use crate::websocket::{TtyMessage, TtyWebSocket};
use axum::extract::ws::WebSocket;
use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteMessage {
    #[serde(rename = "Size")]
    pub size: usize,
    #[serde(rename = "Data")]
    pub data: String, // base64 encoded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinSizeMessage {
    #[serde(rename = "Cols")]
    pub cols: u16,
    #[serde(rename = "Rows")]
    pub rows: u16,
}

pub struct TtyShareSession {
    id: String,
    pty: Arc<Mutex<dyn PtyHandler>>,
    output_tx: broadcast::Sender<TtyMessage>,
}

impl TtyShareSession {
    pub fn new(pty: Arc<Mutex<dyn PtyHandler>>) -> Self {
        let (output_tx, _) = broadcast::channel(1024);

        Self {
            id: Uuid::new_v4().to_string(),
            pty,
            output_tx,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub async fn add_connection(&self, socket: WebSocket) -> Result<()> {
        let tty_ws = Arc::new(Mutex::new(TtyWebSocket::new(socket)));

        // Clone the PTY handler for this connection
        let pty = Arc::clone(&self.pty);

        // Set up output broadcasting
        let mut output_rx = self.output_tx.subscribe();
        let tty_ws_output = Arc::clone(&tty_ws);
        let output_task = tokio::spawn(async move {
            while let Ok(message) = output_rx.recv().await {
                let mut ws = tty_ws_output.lock().await;
                if let Err(e) = ws.send(message).await {
                    error!("Failed to send message to WebSocket: {}", e);
                    break;
                }
            }
        });

        // Set up input handling
        let session_output_tx = self.output_tx.clone();
        let tty_ws_input = Arc::clone(&tty_ws);
        let input_task = tokio::spawn(async move {
            Self::handle_connection_messages(tty_ws_input, pty, session_output_tx).await
        });

        info!("New WebSocket connection added to session {}", self.id);

        // Wait for either task to complete
        tokio::select! {
            _ = output_task => {},
            _ = input_task => {},
        }

        Ok(())
    }

    async fn handle_connection_messages(
        tty_ws: Arc<Mutex<TtyWebSocket>>,
        pty: Arc<Mutex<dyn PtyHandler>>,
        _output_tx: broadcast::Sender<TtyMessage>,
    ) -> Result<()> {
        loop {
            let message = {
                let mut ws = tty_ws.lock().await;
                ws.recv().await
            };

            match message {
                Some(Ok(msg)) => {
                    debug!("Received message: {:?}", msg);
                    match msg.msg_type.as_str() {
                        "Write" => {
                            if let Ok(write_msg_data) = general_purpose::STANDARD.decode(&msg.data)
                            {
                                if let Ok(write_msg) =
                                    serde_json::from_slice::<WriteMessage>(&write_msg_data)
                                {
                                    if let Ok(decoded_data) =
                                        general_purpose::STANDARD.decode(&write_msg.data)
                                    {
                                        let mut pty_guard = pty.lock().await;
                                        if let Err(e) = pty_guard.write(&decoded_data).await {
                                            error!("Failed to write to PTY: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        _ => {
                            debug!("Unknown message type: {}", msg.msg_type);
                        }
                    }
                }
                Some(Err(e)) => {
                    error!("Error receiving WebSocket message: {}", e);
                    break;
                }
                None => {
                    debug!("WebSocket connection closed");
                    break;
                }
            }
        }
        Ok(())
    }

    pub async fn broadcast_output(&self, data: &[u8]) -> Result<()> {
        let write_msg = WriteMessage {
            size: data.len(),
            data: general_purpose::STANDARD.encode(data),
        };

        let message = TtyMessage {
            msg_type: "Write".to_string(),
            data: general_purpose::STANDARD.encode(serde_json::to_vec(&write_msg)?),
        };

        if let Err(e) = self.output_tx.send(message) {
            debug!("No active connections to broadcast to: {}", e);
        }

        Ok(())
    }

    pub async fn broadcast_window_size(&self, cols: u16, rows: u16) -> Result<()> {
        let win_size_msg = WinSizeMessage { cols, rows };

        let message = TtyMessage {
            msg_type: "WinSize".to_string(),
            data: general_purpose::STANDARD.encode(serde_json::to_vec(&win_size_msg)?),
        };

        if let Err(e) = self.output_tx.send(message) {
            debug!("No active connections to broadcast window size to: {}", e);
        }

        Ok(())
    }

    pub async fn refresh(&self) -> Result<()> {
        let mut pty_guard = self.pty.lock().await;
        pty_guard.refresh().await
    }
}
