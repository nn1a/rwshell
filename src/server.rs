use crate::args::Args;
use crate::assets::Assets;
use axum::{
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use base64::{engine::general_purpose, Engine as _};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use terminal_size::{terminal_size, Height, Width};
use termios::{tcsetattr, Termios, TCSANOW};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub session_id: String,
    pub pty_tx: broadcast::Sender<Vec<u8>>,
    pub pty_writer: Arc<Mutex<Option<Box<dyn std::io::Write + Send>>>>,
    pub current_size: Arc<Mutex<(u16, u16)>>, // (cols, rows)
}

#[derive(Serialize, Deserialize)]
struct TtyMessage {
    #[serde(rename = "Type")]
    msg_type: String,
    #[serde(rename = "Data")]
    data: String,
}

#[derive(Serialize, Deserialize)]
struct WriteMessage {
    #[serde(rename = "Size")]
    size: usize,
    #[serde(rename = "Data")]
    data: String,
}

#[derive(Serialize, Deserialize)]
struct WinSizeMessage {
    #[serde(rename = "Cols")]
    cols: u16,
    #[serde(rename = "Rows")]
    rows: u16,
}

pub struct RwShellServer {
    args: Args,
    session_id: String,
}

impl RwShellServer {
    pub async fn new(args: Args) -> anyhow::Result<Self> {
        let session_id = if args.uuid {
            Uuid::new_v4().to_string()
        } else {
            "local".to_string()
        };

        Ok(Self { args, session_id })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        // Display session information
        let url = if self.args.uuid {
            format!("http://{}/s/{}/", self.args.listen, self.session_id)
        } else {
            format!("http://{}/s/local/", self.args.listen)
        };
        println!("local session: {url}");

        // Create PTY with actual terminal size
        let pty_system = native_pty_system();
        let (cols, rows) = if self.args.headless {
            (self.args.headless_cols, self.args.headless_rows)
        } else {
            get_terminal_size()
        };

        let pty_pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Start command
        let mut cmd = CommandBuilder::new(&self.args.command);
        if !self.args.args.is_empty() {
            for arg in self.args.args.split_whitespace() {
                cmd.arg(arg);
            }
        }

        let _child = pty_pair.slave.spawn_command(cmd)?;
        let master = pty_pair.master;

        // Get writer for PTY input
        let pty_writer = master.take_writer()?;

        // Create broadcast channel for PTY output
        let (pty_tx, _) = broadcast::channel(1024);

        // Set up the HTTP server
        let app_state = AppState {
            session_id: self.session_id.clone(),
            pty_tx: pty_tx.clone(),
            pty_writer: Arc::new(Mutex::new(Some(pty_writer))),
            current_size: Arc::new(Mutex::new((cols, rows))),
        };

        let app = self.create_app(app_state.clone()).await?;

        // Set up raw terminal mode for interactive sessions
        let original_termios = if !self.args.headless {
            match setup_raw_terminal() {
                Ok(termios) => Some(termios),
                Err(e) => {
                    debug!("Failed to set raw terminal mode: {}. Continuing anyway.", e);
                    None
                }
            }
        } else {
            None
        };

        // Start the server
        let listener = TcpListener::bind(&self.args.listen).await?;
        debug!("Server listening on: {}", self.args.listen);

        // Start PTY output forwarding in background
        let master_reader = master.try_clone_reader()?;
        let pty_tx_clone = pty_tx.clone();
        let headless = self.args.headless;

        // Create a shutdown signal for when PTY process ends
        let cancellation_token = CancellationToken::new();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let mut shutdown_tx = Some(shutdown_tx);

        let token_clone = cancellation_token.clone();
        let termios_clone = original_termios;
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = master_reader;
            let mut buffer = [0u8; 1024];

            loop {
                match reader.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        let data = buffer[..n].to_vec();

                        // Send to WebSocket clients (ignore error if no subscribers)
                        match pty_tx_clone.send(data.clone()) {
                            Ok(_) => {
                                // Successfully sent to subscribers
                            }
                            Err(tokio::sync::broadcast::error::SendError(_)) => {
                                // No subscribers, which is fine - continue reading
                            }
                        }
                        // Write to stdout if not headless
                        if !headless {
                            print!("{}", String::from_utf8_lossy(&data));
                            use std::io::Write;
                            let _ = std::io::stdout().flush();
                        }
                    }
                    Ok(_) => {
                        debug!("Shell process ended - shutting down server");
                        if let Some(tx) = shutdown_tx.take() {
                            let _ = tx.send(());
                        }
                        token_clone.cancel();

                        // Restore terminal before exiting
                        if let Some(ref termios) = termios_clone {
                            restore_terminal(termios);
                        }

                        // Force immediate exit
                        std::process::exit(0);
                    }
                    Err(e) => {
                        error!("Error reading from PTY: {}", e);
                        if let Some(tx) = shutdown_tx.take() {
                            let _ = tx.send(());
                        }
                        token_clone.cancel();

                        // Restore terminal before exiting
                        if let Some(ref termios) = termios_clone {
                            restore_terminal(termios);
                        }

                        // Force immediate exit
                        std::process::exit(1);
                    }
                }
            }
        });

        // Start terminal size monitoring (if not headless)
        if !self.args.headless {
            let app_state_resize = app_state.clone();
            let pty_tx_resize = pty_tx.clone();
            let token_size = cancellation_token.clone();
            tokio::spawn(async move {
                let mut last_size = (cols, rows);
                let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));

                loop {
                    tokio::select! {
                        _ = token_size.cancelled() => {
                            debug!("Terminal size monitoring task cancelled");
                            break;
                        }
                        _ = interval.tick() => {
                            let current_size = get_terminal_size();

                            if current_size != last_size {
                                debug!("Terminal size changed: {}x{} -> {}x{}",
                                       last_size.0, last_size.1, current_size.0, current_size.1);

                                // Update stored size
                                {
                                    let mut stored_size = app_state_resize.current_size.lock().await;
                                    *stored_size = current_size;
                                }

                                // Send size change to all WebSocket clients
                                let winsize_msg = WinSizeMessage {
                                    cols: current_size.0,
                                    rows: current_size.1,
                                };

                                let tty_msg = TtyMessage {
                                    msg_type: "WinSize".to_string(),
                                    data: general_purpose::STANDARD.encode(serde_json::to_vec(&winsize_msg).unwrap()),
                                };

                                let json_str = serde_json::to_string(&tty_msg).unwrap();

                                // Broadcast to all WebSocket clients via PTY channel
                                // We'll use a special marker to distinguish this from regular PTY output
                                let _ = pty_tx_resize.send(format!("WINSIZE:{json_str}").into_bytes());

                                last_size = current_size;
                            }
                        }
                    }
                }
            });
        }

        // Start stdin forwarding to PTY (if not headless)
        if !self.args.headless {
            let pty_writer_stdin = Arc::clone(&app_state.pty_writer);
            tokio::task::spawn_blocking(move || {
                use std::io::{stdin, Read, Write};
                let mut stdin = stdin();
                let mut buffer = [0u8; 1024];

                loop {
                    match stdin.read(&mut buffer) {
                        Ok(n) if n > 0 => {
                            let data = &buffer[..n];
                            if let Some(writer) = pty_writer_stdin.blocking_lock().as_mut() {
                                let _ = writer.write_all(data);
                                let _ = writer.flush();
                            }
                        }
                        Ok(_) => {
                            eprintln!("Stdin reached EOF");
                            break;
                        }
                        Err(e) => {
                            eprintln!("Error reading from stdin: {e}");
                            break;
                        }
                    }
                }
                eprintln!("Stdin reader task ended");
            });
        }

        // Set up graceful shutdown
        let token_shutdown = cancellation_token.clone();
        let is_headless = self.args.headless;
        let shutdown_signal = async move {
            if is_headless {
                // In headless mode, listen for Ctrl+C to shutdown the server
                tokio::select! {
                    _ = shutdown_rx => {
                        debug!("Shell process ended, shutting down server");
                        token_shutdown.cancel();
                        tokio::spawn(async {
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                            debug!("Exiting rwshell");
                            std::process::exit(0);
                        });
                    }
                    _ = tokio::signal::ctrl_c() => {
                        debug!("Received Ctrl+C in headless mode, shutting down server");
                        token_shutdown.cancel();
                        std::process::exit(0);
                    }
                }
            } else {
                // In interactive mode, only listen for shell process termination
                // Ctrl+C and other signals should go to the shell, not rwshell
                shutdown_rx.await.ok();
                debug!("Shell process ended, shutting down server");
                token_shutdown.cancel();

                // Restore terminal before exiting
                if let Some(ref termios) = original_termios {
                    restore_terminal(termios);
                }

                tokio::spawn(async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    debug!("Exiting rwshell");
                    std::process::exit(0);
                });
            }
        };

        // Start the server with graceful shutdown
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await?;

        Ok(())
    }

    async fn create_app(&self, state: AppState) -> anyhow::Result<Router> {
        let (session_path, static_path, ws_path) = if self.args.uuid {
            (
                format!("/s/{}/", self.session_id),
                format!("/s/{}/static/{{*file}}", self.session_id),
                format!("/s/{}/ws/", self.session_id),
            )
        } else {
            (
                "/s/local/".to_string(),
                "/s/local/static/{*file}".to_string(),
                "/s/local/ws/".to_string(),
            )
        };

        let app = Router::new()
            .route(&session_path, get(serve_session_page))
            .route(&static_path, get(serve_static_file))
            .route(&ws_path, get(handle_websocket))
            .with_state(state);

        Ok(app)
    }
}

fn get_terminal_size() -> (u16, u16) {
    if let Some((Width(w), Height(h))) = terminal_size() {
        (w, h)
    } else {
        // Fallback to default size if unable to detect
        (80, 25)
    }
}

async fn serve_static_file(Path(file): Path<String>) -> Result<Response, StatusCode> {
    match Assets::get_file(&file) {
        Some(content) => {
            let mime_type = Assets::get_content_type(&file);
            Ok(([(header::CONTENT_TYPE, mime_type)], content.data).into_response())
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn serve_session_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    debug!("Serving session page for session: {}", state.session_id);
    match Assets::get_file("rwshell.html") {
        Some(template) => {
            let template_str = String::from_utf8_lossy(&template.data);
            let (path_prefix, ws_path) = if state.session_id == "local" {
                ("/s/local".to_string(), "/s/local/ws/".to_string())
            } else {
                (
                    format!("/s/{}", state.session_id),
                    format!("/s/{}/ws/", state.session_id),
                )
            };

            // Simple template replacement
            let rendered = template_str
                .replace("{{.PathPrefix}}", &path_prefix)
                .replace("{{.WSPath}}", &format!("\"{ws_path}\""));

            Ok(Html(rendered))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn handle_websocket(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    debug!("New WebSocket connection");

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to PTY output
    let mut pty_rx = state.pty_tx.subscribe();

    // Send current terminal size to new client
    {
        let current_size = state.current_size.lock().await;
        let winsize_msg = WinSizeMessage {
            cols: current_size.0,
            rows: current_size.1,
        };

        let message = TtyMessage {
            msg_type: "WinSize".to_string(),
            data: general_purpose::STANDARD.encode(serde_json::to_vec(&winsize_msg).unwrap()),
        };

        let json_str = serde_json::to_string(&message).unwrap();

        if let Err(e) = sender
            .send(axum::extract::ws::Message::Text(json_str.into()))
            .await
        {
            error!("Failed to send initial terminal size: {}", e);
            return;
        }

        debug!(
            "Sent initial terminal size: {}x{}",
            current_size.0, current_size.1
        );
    }

    // Forward PTY output to WebSocket
    let sender_task = tokio::spawn(async move {
        while let Ok(data) = pty_rx.recv().await {
            // Check if this is a WinSize message
            if let Ok(data_str) = String::from_utf8(data.clone()) {
                if let Some(winsize_json) = data_str.strip_prefix("WINSIZE:") {
                    // Extract and send the WinSize message directly
                    // Remove "WINSIZE:" prefix
                    if let Err(e) = sender
                        .send(axum::extract::ws::Message::Text(
                            winsize_json.to_string().into(),
                        ))
                        .await
                    {
                        error!("Failed to send WinSize message: {}", e);
                        break;
                    }
                    continue;
                }
            }

            debug!("Sending {} bytes to WebSocket", data.len());

            let write_msg = WriteMessage {
                size: data.len(),
                data: general_purpose::STANDARD.encode(&data),
            };

            let message = TtyMessage {
                msg_type: "Write".to_string(),
                data: general_purpose::STANDARD.encode(serde_json::to_vec(&write_msg).unwrap()),
            };

            let json_str = serde_json::to_string(&message).unwrap();

            if let Err(e) = sender
                .send(axum::extract::ws::Message::Text(json_str.into()))
                .await
            {
                error!("Failed to send WebSocket message: {}", e);
                break;
            }
        }
        debug!("PTY to WebSocket sender task ended");
    });

    // Handle WebSocket input
    let pty_writer = state.pty_writer;
    let receiver_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            if let Ok(axum::extract::ws::Message::Text(text)) = msg {
                debug!("Received WebSocket message: {} chars", text.len());
                if let Ok(tty_msg) = serde_json::from_str::<TtyMessage>(&text) {
                    if tty_msg.msg_type == "Write" {
                        if let Ok(write_msg_data) = general_purpose::STANDARD.decode(&tty_msg.data)
                        {
                            if let Ok(write_msg) =
                                serde_json::from_slice::<WriteMessage>(&write_msg_data)
                            {
                                if let Ok(decoded_data) =
                                    general_purpose::STANDARD.decode(&write_msg.data)
                                {
                                    debug!(
                                        "Writing {} bytes to PTY: {:?}",
                                        decoded_data.len(),
                                        String::from_utf8_lossy(&decoded_data)
                                    );
                                    if let Some(writer) = pty_writer.lock().await.as_mut() {
                                        use std::io::Write;
                                        let _ = writer.write_all(&decoded_data);
                                        let _ = writer.flush();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        debug!("WebSocket receiver task ended");
    });

    // Wait for either task to complete
    tokio::select! {
        _ = sender_task => {},
        _ = receiver_task => {},
    }

    debug!("WebSocket connection closed");
}

fn setup_raw_terminal() -> Result<Termios, std::io::Error> {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let original_termios = Termios::from_fd(stdin_fd)?;
    let mut raw_termios = original_termios;

    // Set raw mode
    termios::cfmakeraw(&mut raw_termios);

    // Apply the raw terminal settings
    tcsetattr(stdin_fd, TCSANOW, &raw_termios)?;

    Ok(original_termios)
}

fn restore_terminal(original_termios: &Termios) {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let _ = tcsetattr(stdin_fd, TCSANOW, original_termios);
}
