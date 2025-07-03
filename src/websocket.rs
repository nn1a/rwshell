use crate::error::{Result, RwShellError};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, error};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtyMessage {
    #[serde(rename = "Type")]
    pub msg_type: String,
    #[serde(rename = "Data")]
    pub data: String, // base64 encoded
}

pub struct TtyWebSocket {
    socket: WebSocket,
}

impl TtyWebSocket {
    pub fn new(socket: WebSocket) -> Self {
        Self { socket }
    }

    pub async fn recv(&mut self) -> Option<Result<TtyMessage>> {
        match self.socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<TtyMessage>(&text) {
                    Ok(msg) => Some(Ok(msg)),
                    Err(e) => Some(Err(RwShellError::Json(e))),
                }
            }
            Some(Ok(Message::Binary(data))) => {
                match serde_json::from_slice::<TtyMessage>(&data) {
                    Ok(msg) => Some(Ok(msg)),
                    Err(e) => Some(Err(RwShellError::Json(e))),
                }
            }
            Some(Ok(Message::Close(_))) => {
                debug!("WebSocket connection closed");
                None
            }
            Some(Err(e)) => {
                error!("WebSocket error: {:?}", e);
                None
            }
            None => None,
            _ => {
                debug!("Received non-text/binary WebSocket message");
                // Continue receiving for other message types
                self.recv().await
            }
        }
    }

    pub async fn send(&mut self, message: TtyMessage) -> Result<()> {
        let json_str = serde_json::to_string(&message)?;
        self.socket.send(Message::Text(json_str)).await
            .map_err(|e| RwShellError::Server(format!("Failed to send WebSocket message: {:?}", e)))?;
        Ok(())
    }
}
