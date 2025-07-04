use crate::args::Args;
use crate::assets::Assets;
use axum::{
    Router,
    extract::{
        Path, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use terminal_size::{Height, Width, terminal_size};
use termios::{TCSANOW, Termios, tcsetattr};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub session_id: String,
    pub pty_tx: broadcast::Sender<Vec<u8>>,
    pub pty_writer: Arc<Mutex<Option<Box<dyn std::io::Write + Send>>>>,
    pub pty_master: Arc<Mutex<Box<dyn MasterPty + Send>>>, // Add PTY master for resizing
    pub current_size: Arc<Mutex<(u16, u16)>>,              // (cols, rows)
    pub output_buffer: Arc<Mutex<Vec<u8>>>,                // Buffer for output before client connects
    pub readonly: bool,                                    // Whether session is read-only
    pub headless: bool,                                    // Whether server is in headless mode
    pub last_resize_time: Arc<Mutex<std::time::Instant>>,  // For rate limiting resize requests
    pub pending_resize: Arc<Mutex<Option<(u16, u16)>>>,    // Store pending resize request
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

#[derive(Serialize, Deserialize)]
struct ReadOnlyMessage {
    #[serde(rename = "ReadOnly")]
    readonly: bool,
}

#[derive(Serialize, Deserialize)]
struct HeadlessMessage {
    #[serde(rename = "Headless")]
    headless: bool,
}

/// Validates terminal size to prevent abuse or invalid values
fn is_valid_terminal_size(cols: u16, rows: u16) -> bool {
    // Minimum reasonable terminal size
    const MIN_COLS: u16 = 10;
    const MIN_ROWS: u16 = 5;

    // Maximum reasonable terminal size (prevent memory/resource abuse)
    const MAX_COLS: u16 = 1000;
    const MAX_ROWS: u16 = 1000;

    // Check for zero values (invalid)
    if cols == 0 || rows == 0 {
        return false;
    }

    // Check bounds
    (MIN_COLS..=MAX_COLS).contains(&cols) && (MIN_ROWS..=MAX_ROWS).contains(&rows)
}

/// Process resize request with rate limiting and pending request handling
async fn process_resize_request(
    cols: u16,
    rows: u16,
    last_resize_time: &Arc<Mutex<std::time::Instant>>,
    pending_resize: &Arc<Mutex<Option<(u16, u16)>>>,
    pty_master: &Arc<Mutex<Box<dyn MasterPty + Send>>>,
    current_size: &Arc<Mutex<(u16, u16)>>,
    pty_tx: &broadcast::Sender<Vec<u8>>,
) -> bool {
    const MIN_RESIZE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

    let now = std::time::Instant::now();
    let should_apply_immediately = {
        let mut last_time = last_resize_time.lock().await;
        if now.duration_since(*last_time) >= MIN_RESIZE_INTERVAL {
            *last_time = now;
            true
        } else {
            false
        }
    };

    if should_apply_immediately {
        // Apply the resize immediately
        apply_resize(cols, rows, pty_master, current_size, pty_tx).await;
        true
    } else {
        // Store as pending resize (overwrites any previous pending)
        {
            let mut pending_lock = pending_resize.lock().await;
            *pending_lock = Some((cols, rows));
        }
        debug!(
            "Rate limiting: storing resize request as pending: {}x{} ({}ms since last)",
            cols,
            rows,
            now.duration_since(*last_resize_time.lock().await).as_millis()
        );
        false
    }
}

/// Apply resize immediately without rate limiting
async fn apply_resize(
    cols: u16,
    rows: u16,
    pty_master: &Arc<Mutex<Box<dyn MasterPty + Send>>>,
    current_size: &Arc<Mutex<(u16, u16)>>,
    pty_tx: &broadcast::Sender<Vec<u8>>,
) {
    // Update stored size
    {
        let mut stored_size = current_size.lock().await;
        *stored_size = (cols, rows);
    }

    // Resize the PTY
    {
        let pty_master_lock = pty_master.lock().await;
        let new_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        if let Err(e) = pty_master_lock.resize(new_size) {
            error!("Failed to resize PTY: {}", e);
        } else {
            debug!("Successfully resized PTY to {}x{}", cols, rows);
        }
    }

    // Broadcast size change to other WebSocket clients
    let winsize_msg = WinSizeMessage { cols, rows };
    let tty_msg_broadcast = TtyMessage {
        msg_type: "WinSize".to_string(),
        data: general_purpose::STANDARD.encode(serde_json::to_vec(&winsize_msg).unwrap()),
    };

    let json_str = serde_json::to_string(&tty_msg_broadcast).unwrap();
    let _ = pty_tx.send(format!("WINSIZE:{json_str}").into_bytes());
}

/// Start a background task to process pending resize requests
fn start_pending_resize_processor(
    last_resize_time: Arc<Mutex<std::time::Instant>>,
    pending_resize: Arc<Mutex<Option<(u16, u16)>>>,
    pty_master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    current_size: Arc<Mutex<(u16, u16)>>,
    pty_tx: broadcast::Sender<Vec<u8>>,
    cancellation_token: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        const MIN_RESIZE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

        let mut interval = tokio::time::interval(CHECK_INTERVAL);

        loop {
            tokio::select! {
                _ = cancellation_token.cancelled() => {
                    debug!("Pending resize processor cancelled");
                    break;
                }
                _ = interval.tick() => {
                    // Check if we have a pending resize and enough time has passed
                    let pending = {
                        let pending_lock = pending_resize.lock().await;
                        *pending_lock
                    };

                    if let Some((cols, rows)) = pending {
                        let now = std::time::Instant::now();
                        let last_time = *last_resize_time.lock().await;

                        if now.duration_since(last_time) >= MIN_RESIZE_INTERVAL {
                            // Clear the pending resize
                            {
                                let mut pending_lock = pending_resize.lock().await;
                                *pending_lock = None;
                            }

                            // Update last resize time
                            {
                                let mut last_time_lock = last_resize_time.lock().await;
                                *last_time_lock = now;
                            }

                            debug!("Processing pending resize: {}x{}", cols, rows);
                            apply_resize(cols, rows, &pty_master, &current_size, &pty_tx).await;
                        }
                    }
                }
            }
        }
    });
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

        // Validate initial terminal size
        if !is_valid_terminal_size(cols, rows) {
            return Err(anyhow::anyhow!(
                "Invalid initial terminal size: {}x{} (must be between {}x{} and {}x{})",
                cols,
                rows,
                10,
                5,
                1000,
                1000
            ));
        }

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

        // set RWSHELL environment variable to indicate we're in rwshell
        cmd.env("RWSHELL", "1");
        cmd.env("RWSHELL_SESSION", &self.session_id);

        let mut child = pty_pair.slave.spawn_command(cmd)?;
        let master = pty_pair.master;

        // Get writer for PTY input
        let pty_writer = master.take_writer()?;

        // Clone master for reading before moving it to AppState
        let master_reader = master.try_clone_reader()?;

        // Create broadcast channel for PTY output
        let (pty_tx, _) = broadcast::channel(1024);

        // Set up the HTTP server
        let app_state = AppState {
            session_id: self.session_id.clone(),
            pty_tx: pty_tx.clone(),
            pty_writer: Arc::new(Mutex::new(Some(pty_writer))),
            pty_master: Arc::new(Mutex::new(master)),
            current_size: Arc::new(Mutex::new((cols, rows))),
            output_buffer: Arc::new(Mutex::new(Vec::new())),
            readonly: self.args.readonly,
            headless: self.args.headless,
            last_resize_time: Arc::new(Mutex::new(std::time::Instant::now())),
            pending_resize: Arc::new(Mutex::new(None)),
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
        let pty_tx_clone = pty_tx.clone();
        let headless = self.args.headless;

        // Create a shutdown signal for when PTY process ends
        let cancellation_token = CancellationToken::new();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let (child_shutdown_tx, child_shutdown_rx) = tokio::sync::oneshot::channel();
        let mut shutdown_tx = Some(shutdown_tx);

        // Start pending resize processor for headless mode
        if self.args.headless {
            start_pending_resize_processor(
                app_state.last_resize_time.clone(),
                app_state.pending_resize.clone(),
                app_state.pty_master.clone(),
                app_state.current_size.clone(),
                pty_tx.clone(),
                cancellation_token.clone(),
            );
        }

        // Monitor child process to prevent zombie processes
        let token_child = cancellation_token.clone();
        tokio::task::spawn_blocking(move || {
            loop {
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        debug!("Child process exited with status: {:?}", exit_status);
                        let _ = child_shutdown_tx.send(());
                        token_child.cancel();
                        break;
                    }
                    Ok(None) => {
                        // Process is still running, check cancellation and continue
                        if token_child.is_cancelled() {
                            debug!("Child monitor task cancelled");
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    Err(e) => {
                        error!("Error checking child process status: {}", e);
                        let _ = child_shutdown_tx.send(());
                        token_child.cancel();
                        break;
                    }
                }
            }
        });

        let token_clone = cancellation_token.clone();
        let termios_clone = original_termios;
        let app_state_buffer = app_state.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut reader = master_reader;
            let mut buffer = [0u8; 1024];

            loop {
                match reader.read(&mut buffer) {
                    Ok(n) if n > 0 => {
                        let data = buffer[..n].to_vec();

                        // Check if there are any subscribers
                        let has_subscribers = pty_tx_clone.receiver_count() > 0;

                        if has_subscribers {
                            // Send to WebSocket clients
                            match pty_tx_clone.send(data.clone()) {
                                Ok(_) => {
                                    // Successfully sent to subscribers
                                }
                                Err(tokio::sync::broadcast::error::SendError(_)) => {
                                    // No subscribers, which shouldn't happen here but handle gracefully
                                }
                            }
                        } else {
                            // No subscribers, buffer the data (up to 1KB)
                            let mut output_buffer = app_state_buffer.output_buffer.blocking_lock();
                            output_buffer.extend_from_slice(&data);

                            // Keep only the last 1KB of data
                            const MAX_BUFFER_SIZE: usize = 1024;
                            if output_buffer.len() > MAX_BUFFER_SIZE {
                                let start = output_buffer.len() - MAX_BUFFER_SIZE;
                                output_buffer.drain(0..start);
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

                                // Validate the new terminal size before applying it
                                if !is_valid_terminal_size(current_size.0, current_size.1) {
                                    debug!("Ignoring invalid terminal size from host terminal: {}x{}",
                                           current_size.0, current_size.1);
                                    continue;
                                }

                                // Update stored size
                                {
                                    let mut stored_size = app_state_resize.current_size.lock().await;
                                    *stored_size = current_size;
                                }

                                // Resize the PTY to match new terminal size
                                {
                                    let pty_master = app_state_resize.pty_master.lock().await;
                                    let new_size = PtySize {
                                        rows: current_size.1,
                                        cols: current_size.0,
                                        pixel_width: 0,
                                        pixel_height: 0,
                                    };

                                    if let Err(e) = pty_master.resize(new_size) {
                                        error!("Failed to resize PTY: {}", e);
                                    } else {
                                        debug!("Successfully resized PTY to {}x{}", current_size.0, current_size.1);
                                    }
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
                use std::io::{Read, Write, stdin};
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
                    _ = child_shutdown_rx => {
                        debug!("Child process ended, shutting down server");
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
                // In interactive mode, listen for shell or child process termination
                tokio::select! {
                    _ = shutdown_rx => {
                        debug!("Shell process ended, shutting down server");
                    }
                    _ = child_shutdown_rx => {
                        debug!("Child process ended, shutting down server");
                    }
                }
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
            .fallback(serve_404)
            .with_state(state);

        Ok(app)
    }
}

async fn serve_404() -> Response {
    match Assets::get_file("404.html") {
        Some(content) => {
            let content_str = String::from_utf8_lossy(&content.data);
            (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                content_str.to_string(),
            )
                .into_response()
        }
        None => {
            // Fallback if 404.html is not found
            (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain")],
                "404 - Page Not Found".to_string(),
            )
                .into_response()
        }
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

async fn serve_static_file(Path(file): Path<String>) -> Response {
    match Assets::get_file(&file) {
        Some(content) => {
            let mime_type = Assets::get_content_type(&file);
            ([(header::CONTENT_TYPE, mime_type)], content.data).into_response()
        }
        None => {
            // Serve 404.html with 404 status code for missing static files
            match Assets::get_file("404.html") {
                Some(content) => {
                    let content_str = String::from_utf8_lossy(&content.data);
                    (
                        StatusCode::NOT_FOUND,
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        content_str.to_string(),
                    )
                        .into_response()
                }
                None => {
                    // Fallback if 404.html is not found
                    (
                        StatusCode::NOT_FOUND,
                        [(header::CONTENT_TYPE, "text/plain")],
                        "404 - Static File Not Found".to_string(),
                    )
                        .into_response()
                }
            }
        }
    }
}

async fn serve_session_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    debug!("Serving session page for session: {}", state.session_id);
    match Assets::get_file("index.html") {
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
                .replace("__PathPrefix__", &path_prefix)
                .replace("__WSPath__", &format!("\"{ws_path}\""));

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

        if let Err(e) = sender.send(axum::extract::ws::Message::Text(json_str.into())).await {
            let error_msg = e.to_string();
            if error_msg.contains("closed connection")
                || error_msg.contains("Connection reset")
                || error_msg.contains("Trying to work with closed connection")
            {
                debug!("WebSocket connection closed while sending initial terminal size: {}", e);
            } else {
                error!("Failed to send initial terminal size: {}", e);
            }
            return;
        }

        debug!("Sent initial terminal size: {}x{}", current_size.0, current_size.1);
    }

    // Send readonly state to new client
    {
        let readonly_msg = ReadOnlyMessage {
            readonly: state.readonly,
        };

        let message = TtyMessage {
            msg_type: "ReadOnly".to_string(),
            data: general_purpose::STANDARD.encode(serde_json::to_vec(&readonly_msg).unwrap()),
        };

        let json_str = serde_json::to_string(&message).unwrap();

        if let Err(e) = sender.send(axum::extract::ws::Message::Text(json_str.into())).await {
            let error_msg = e.to_string();
            if error_msg.contains("closed connection")
                || error_msg.contains("Connection reset")
                || error_msg.contains("Trying to work with closed connection")
            {
                debug!("WebSocket connection closed while sending readonly state: {}", e);
            } else {
                error!("Failed to send readonly state: {}", e);
            }
            return;
        }

        debug!("Sent readonly state: {}", state.readonly);
    }

    // Send headless state to new client
    {
        let headless_msg = HeadlessMessage {
            headless: state.headless,
        };

        let message = TtyMessage {
            msg_type: "Headless".to_string(),
            data: general_purpose::STANDARD.encode(serde_json::to_vec(&headless_msg).unwrap()),
        };

        let json_str = serde_json::to_string(&message).unwrap();

        if let Err(e) = sender.send(axum::extract::ws::Message::Text(json_str.into())).await {
            let error_msg = e.to_string();
            if error_msg.contains("closed connection")
                || error_msg.contains("Connection reset")
                || error_msg.contains("Trying to work with closed connection")
            {
                debug!("WebSocket connection closed while sending headless state: {}", e);
            } else {
                error!("Failed to send headless state: {}", e);
            }
            return;
        }

        debug!("Sent headless state: {}", state.headless);
    }

    // Send buffered output to new client
    {
        let mut output_buffer = state.output_buffer.lock().await;
        if !output_buffer.is_empty() {
            debug!("Sending {} bytes of buffered output to new client", output_buffer.len());

            let write_msg = WriteMessage {
                size: output_buffer.len(),
                data: general_purpose::STANDARD.encode(&*output_buffer),
            };

            let message = TtyMessage {
                msg_type: "Write".to_string(),
                data: general_purpose::STANDARD.encode(serde_json::to_vec(&write_msg).unwrap()),
            };

            let json_str = serde_json::to_string(&message).unwrap();

            if let Err(e) = sender.send(axum::extract::ws::Message::Text(json_str.into())).await {
                // 연결이 닫힌 경우는 정상적인 상황이므로 debug 레벨로 로깅
                let error_msg = e.to_string();
                if error_msg.contains("closed connection")
                    || error_msg.contains("Connection reset")
                    || error_msg.contains("Trying to work with closed connection")
                {
                    debug!("WebSocket connection closed while sending buffered output: {}", e);
                } else {
                    error!("Failed to send buffered output: {}", e);
                }
                return;
            }

            // Clear the buffer after sending
            output_buffer.clear();
        }
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
                        .send(axum::extract::ws::Message::Text(winsize_json.to_string().into()))
                        .await
                    {
                        let error_msg = e.to_string();
                        if error_msg.contains("closed connection")
                            || error_msg.contains("Connection reset")
                            || error_msg.contains("Trying to work with closed connection")
                        {
                            debug!("WebSocket connection closed while sending WinSize: {}", e);
                        } else {
                            error!("Failed to send WinSize message: {}", e);
                        }
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

            if let Err(e) = sender.send(axum::extract::ws::Message::Text(json_str.into())).await {
                let error_msg = e.to_string();
                if error_msg.contains("closed connection")
                    || error_msg.contains("Connection reset")
                    || error_msg.contains("Trying to work with closed connection")
                {
                    debug!("WebSocket connection closed: {}", e);
                } else {
                    error!("Failed to send WebSocket message: {}", e);
                }
                break;
            }
        }
        debug!("PTY to WebSocket sender task ended");
    });

    // Handle WebSocket input
    let pty_writer = state.pty_writer;
    let readonly = state.readonly;
    let headless = state.headless;
    let pty_master_for_resize = state.pty_master;
    let current_size_for_resize = state.current_size;
    let pty_tx_for_resize = state.pty_tx;
    let last_resize_time = state.last_resize_time;
    let pending_resize = state.pending_resize;
    let receiver_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            if let Ok(axum::extract::ws::Message::Text(text)) = msg {
                debug!("Received WebSocket message: {} chars", text.len());
                if let Ok(tty_msg) = serde_json::from_str::<TtyMessage>(&text) {
                    if tty_msg.msg_type == "Write" {
                        // Ignore input if session is read-only
                        if readonly {
                            debug!("Ignoring input in read-only mode");
                            continue;
                        }

                        if let Ok(write_msg_data) = general_purpose::STANDARD.decode(&tty_msg.data) {
                            if let Ok(write_msg) = serde_json::from_slice::<WriteMessage>(&write_msg_data) {
                                if let Ok(decoded_data) = general_purpose::STANDARD.decode(&write_msg.data) {
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
                    } else if tty_msg.msg_type == "WinSize" && headless {
                        // Only process WinSize messages from clients in headless mode
                        if let Ok(winsize_data) = general_purpose::STANDARD.decode(&tty_msg.data) {
                            if let Ok(winsize_msg) = serde_json::from_slice::<WinSizeMessage>(&winsize_data) {
                                // Validate terminal size to prevent abuse
                                if !is_valid_terminal_size(winsize_msg.cols, winsize_msg.rows) {
                                    debug!(
                                        "Rejected invalid terminal size from client: {}x{} (outside valid range)",
                                        winsize_msg.cols, winsize_msg.rows
                                    );
                                    continue;
                                }

                                debug!(
                                    "Received WinSize from client in headless mode: {}x{}",
                                    winsize_msg.cols, winsize_msg.rows
                                );

                                // Process the resize request with rate limiting
                                let applied = process_resize_request(
                                    winsize_msg.cols,
                                    winsize_msg.rows,
                                    &last_resize_time,
                                    &pending_resize,
                                    &pty_master_for_resize,
                                    &current_size_for_resize,
                                    &pty_tx_for_resize,
                                )
                                .await;

                                if applied {
                                    debug!("Resize applied immediately: {}x{}", winsize_msg.cols, winsize_msg.rows);
                                } else {
                                    debug!("Resize stored as pending: {}x{}", winsize_msg.cols, winsize_msg.rows);
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
