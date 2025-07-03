use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TtyMessage {
    #[serde(rename = "Type")]
    msg_type: String,
    #[serde(rename = "Data")]
    data: String, // base64 encoded
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriteMessage {
    #[serde(rename = "Size")]
    size: usize,
    #[serde(rename = "Data")]
    data: String, // base64 encoded
}

#[derive(Parser, Debug)]
#[command(name = "rwshell-client")]
#[command(about = "Connect to a rwshell session")]
struct ClientArgs {
    /// The session URL to connect to
    #[arg(help = "Session URL (e.g. http://localhost:8000/s/local/)")]
    session_url: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

async fn run_client(session_url: String) -> Result<()> {
    // Parse the session URL and convert to WebSocket URL
    let url = Url::parse(&session_url)?;

    let ws_scheme = if url.scheme() == "https" { "wss" } else { "ws" };

    // Build host with port
    let host_port = if let Some(port) = url.port() {
        format!("{}:{}", url.host_str().unwrap_or("localhost"), port)
    } else {
        url.host_str().unwrap_or("localhost").to_string()
    };

    // Build WebSocket URL - append "ws" to the path
    let mut path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("ws/") {
        path.push_str("/ws/");
    }

    let ws_url = format!("{ws_scheme}://{host_port}{path}");

    debug!("Connecting to WebSocket: {}", ws_url);

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
                Ok(_) => {
                    debug!("Stdin reached EOF");
                    break;
                }
                Err(e) => {
                    error!("Error reading from stdin: {}", e);
                    break;
                }
            }
        }
        debug!("Stdin forwarding task ended");
    });

    // Set up stdout output
    let stdout_task = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();

        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(tty_msg) = serde_json::from_str::<TtyMessage>(&text) {
                        if tty_msg.msg_type == "Write" {
                            if let Ok(data) = general_purpose::STANDARD.decode(&tty_msg.data) {
                                if let Ok(write_msg) = serde_json::from_slice::<WriteMessage>(&data)
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
                    debug!("WebSocket connection closed");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {:?}", e);
                    break;
                }
                _ => {
                    // Ignore other message types
                }
            }
        }
        debug!("Stdout forwarding task ended");
    });

    // Wait for either task to complete
    tokio::select! {
        _ = stdin_task => {},
        _ = stdout_task => {},
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = ClientArgs::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(format!("rwshell_client={log_level}"))
        .init();

    // Run client
    if let Err(e) = run_client(args.session_url).await {
        error!("Client error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}
