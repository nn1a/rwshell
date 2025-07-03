use crate::error::{Result, RwShellError};
use crate::session::WriteMessage;
use crate::websocket::TtyMessage;
use base64::{engine::general_purpose, Engine as _};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info};
use url::Url;

pub struct TtyClient {
    session_url: String,
    #[allow(dead_code)]
    detach_keys: String,
}

impl TtyClient {
    pub fn new(session_url: String, detach_keys: String) -> Result<Self> {
        Ok(Self {
            session_url,
            detach_keys,
        })
    }

    pub async fn run(&self) -> Result<()> {
        // Parse the session URL and convert to WebSocket URL
        let url = Url::parse(&self.session_url)
            .map_err(|e| RwShellError::InvalidUrl(format!("Invalid URL: {e}")))?;

        let ws_scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        let ws_url = format!(
            "{}://{}/ws",
            ws_scheme,
            url.host_str().unwrap_or("localhost")
        );

        info!("Connecting to WebSocket: {}", ws_url);

        let (ws_stream, _) = connect_async(&ws_url).await?;
        let (mut ws_sender, mut ws_receiver) = ws_stream.split();

        // Set up stdin forwarding
        let stdin_task = tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buffer = [0u8; 1024];

            loop {
                match stdin.read(&mut buffer).await {
                    Ok(n) if n > 0 => {
                        let data = general_purpose::STANDARD.encode(&buffer[..n]);
                        let write_msg = WriteMessage { size: n, data };

                        let message = TtyMessage {
                            msg_type: "Write".to_string(),
                            data: general_purpose::STANDARD
                                .encode(serde_json::to_vec(&write_msg).unwrap()),
                        };

                        let json_str = serde_json::to_string(&message).unwrap();

                        if let Err(e) = ws_sender.send(Message::Text(json_str)).await {
                            error!("Failed to send message: {}", e);
                            break;
                        }
                    }
                    Ok(_) => break, // EOF
                    Err(e) => {
                        error!("Failed to read from stdin: {}", e);
                        break;
                    }
                }
            }
        });

        // Set up stdout forwarding
        let stdout_task = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();

            while let Some(msg) = ws_receiver.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(tty_msg) = serde_json::from_str::<TtyMessage>(&text) {
                            if tty_msg.msg_type == "Write" {
                                if let Ok(data) = general_purpose::STANDARD.decode(&tty_msg.data) {
                                    if let Ok(write_msg) =
                                        serde_json::from_slice::<WriteMessage>(&data)
                                    {
                                        if let Ok(output) =
                                            general_purpose::STANDARD.decode(&write_msg.data)
                                        {
                                            if let Err(e) = stdout.write_all(&output).await {
                                                error!("Failed to write to stdout: {}", e);
                                                break;
                                            }
                                            if let Err(e) = stdout.flush().await {
                                                error!("Failed to flush stdout: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("WebSocket connection closed");
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        });

        // Wait for either task to complete
        tokio::select! {
            _ = stdin_task => {},
            _ = stdout_task => {},
        }

        Ok(())
    }
}
