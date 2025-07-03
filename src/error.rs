use thiserror::Error;

#[derive(Error, Debug)]
pub enum RwShellError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("PTY error: {0}")]
    Pty(String),

    #[error("Server error: {0}")]
    Server(String),

    #[error("Client error: {0}")]
    Client(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Connection closed")]
    ConnectionClosed,
}

pub type Result<T> = std::result::Result<T, RwShellError>;
