use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use termios::{tcsetattr, Termios};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error};
use url::Url;

// Global state for terminal restoration
static mut ORIGINAL_TERMIOS: Option<Termios> = None;
static TERMIOS_INITIALIZED: AtomicBool = AtomicBool::new(false);
static TERMIOS_MUTEX: Mutex<()> = Mutex::new(());

// Global terminal restoration function
extern "C" fn global_restore_terminal() {
    unsafe {
        if TERMIOS_INITIALIZED.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(ref termios) = ORIGINAL_TERMIOS {
                if let Ok(_lock) = TERMIOS_MUTEX.lock() {
                    restore_terminal_internal(termios);
                }
            }
        }
    }
}

// Internal terminal restoration function
fn restore_terminal_internal(original_termios: &Termios) {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    let stderr_fd = std::io::stderr().as_raw_fd();

    let _ = tcsetattr(stdin_fd, termios::TCSAFLUSH, original_termios);
    let _ = tcsetattr(stdout_fd, termios::TCSAFLUSH, original_termios);
    let _ = tcsetattr(stderr_fd, termios::TCSAFLUSH, original_termios);
}

// Set up global terminal restoration handlers
fn setup_global_terminal_restoration(original_termios: Termios) -> Result<()> {
    unsafe {
        let _lock = TERMIOS_MUTEX.lock().unwrap();
        ORIGINAL_TERMIOS = Some(original_termios);
        TERMIOS_INITIALIZED.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    // Set up atexit handler for normal program termination
    extern "C" {
        fn atexit(f: extern "C" fn()) -> i32;
    }

    unsafe {
        atexit(global_restore_terminal);
    }

    // Set up signal handlers for various termination signals
    unsafe {
        libc::signal(libc::SIGINT, global_restore_terminal as usize); // Ctrl+C
        libc::signal(libc::SIGTERM, global_restore_terminal as usize); // Termination request
        libc::signal(libc::SIGHUP, global_restore_terminal as usize); // Hangup
        libc::signal(libc::SIGQUIT, global_restore_terminal as usize); // Quit
        libc::signal(libc::SIGABRT, global_restore_terminal as usize); // Abort
    }

    Ok(())
}

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

// Structure for window size (from sys/ioctl.h)
#[repr(C)]
struct WinSize {
    ws_row: libc::c_ushort,    // rows, in characters
    ws_col: libc::c_ushort,    // columns, in characters
    ws_xpixel: libc::c_ushort, // horizontal size, pixels
    ws_ypixel: libc::c_ushort, // vertical size, pixels
}

// Function to set terminal window size
fn set_terminal_size(cols: u16, rows: u16) -> Result<()> {
    use std::os::unix::io::AsRawFd;

    let stdout_fd = std::io::stdout().as_raw_fd();

    let winsize = WinSize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    unsafe {
        let result = libc::ioctl(stdout_fd, libc::TIOCSWINSZ, &winsize);
        if result == -1 {
            return Err(anyhow::anyhow!("Failed to set terminal size"));
        }
    }

    debug!("Terminal size set to {}x{}", cols, rows);
    Ok(())
}

// Function to get current terminal size
fn get_terminal_size() -> Result<(u16, u16)> {
    use std::os::unix::io::AsRawFd;

    let stdout_fd = std::io::stdout().as_raw_fd();

    let mut winsize = WinSize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    unsafe {
        let result = libc::ioctl(stdout_fd, libc::TIOCGWINSZ, &mut winsize);
        if result == -1 {
            return Err(anyhow::anyhow!("Failed to get terminal size"));
        }
    }

    Ok((winsize.ws_col, winsize.ws_row))
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
    // Set up raw terminal mode to prevent local echo
    let original_termios = setup_raw_terminal()?;

    // Set up global terminal restoration for all exit scenarios
    setup_global_terminal_restoration(original_termios)?;

    // Get initial terminal size
    let (initial_cols, initial_rows) = get_terminal_size().unwrap_or((80, 24));
    debug!("Initial terminal size: {}x{}", initial_cols, initial_rows);

    // Create an atomic flag for graceful shutdown
    let shutdown_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

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

    let shutdown_flag_for_stdin = shutdown_flag.clone();

    // Set up stdin forwarding using blocking I/O to capture each keystroke
    let stdin_task = tokio::task::spawn_blocking(move || {
        use std::io::{stdin, Read};
        let mut stdin = stdin();
        let mut buffer = [0u8; 1]; // Read one byte at a time for immediate response

        loop {
            // Check shutdown flag
            if shutdown_flag_for_stdin.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            match stdin.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    // Check for Ctrl+C (ASCII 3) to exit client
                    if buffer[0] == 3 {
                        debug!("Ctrl+C detected, exiting client");
                        // Set shutdown flag and break from loop
                        shutdown_flag_for_stdin.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }

                    let data = general_purpose::STANDARD.encode(&buffer[..n]);
                    let write_msg = WriteMessage { size: n, data };

                    let message = TtyMessage {
                        msg_type: "Write".to_string(),
                        data: general_purpose::STANDARD
                            .encode(serde_json::to_vec(&write_msg).unwrap()),
                    };

                    let json_str = serde_json::to_string(&message).unwrap();

                    // Send message synchronously using tokio runtime handle
                    let rt = tokio::runtime::Handle::current();
                    if let Err(e) = rt.block_on(ws_sender.send(Message::Text(json_str))) {
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

    let shutdown_flag_for_stdout = shutdown_flag.clone();

    // Set up stdout output using blocking I/O for immediate display
    let stdout_task = tokio::spawn(async move {
        use std::io::{stdout, Write};
        let mut stdout = stdout();

        while let Some(msg) = ws_receiver.next().await {
            // Check shutdown flag
            if shutdown_flag_for_stdout.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

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
                                        // Write directly to stdout without buffering for immediate display
                                        if let Err(e) = stdout.write_all(&output) {
                                            error!("Failed to write to stdout: {}", e);
                                            break;
                                        }
                                        if let Err(e) = stdout.flush() {
                                            error!("Failed to flush stdout: {}", e);
                                        }
                                    }
                                }
                            }
                        } else if tty_msg.msg_type == "WinSize" {
                            // Handle window size changes
                            if let Ok(data) = general_purpose::STANDARD.decode(&tty_msg.data) {
                                if let Ok(winsize_msg) =
                                    serde_json::from_slice::<serde_json::Value>(&data)
                                {
                                    if let (Some(cols), Some(rows)) = (
                                        winsize_msg.get("Cols").and_then(|v| v.as_u64()),
                                        winsize_msg.get("Rows").and_then(|v| v.as_u64()),
                                    ) {
                                        debug!("Received window size change: {}x{}", cols, rows);
                                        // Set the actual terminal size
                                        if let Err(e) = set_terminal_size(cols as u16, rows as u16)
                                        {
                                            error!("Failed to set terminal size: {}", e);
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

    // Wait for either task to complete or shutdown flag
    tokio::select! {
        _ = stdin_task => {
            debug!("Stdin task completed");
        },
        _ = stdout_task => {
            debug!("Stdout task completed");
        },
    }

    // Restore terminal before exiting
    restore_terminal(&original_termios);

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

fn setup_raw_terminal() -> Result<Termios> {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    let stderr_fd = std::io::stderr().as_raw_fd();

    let original_termios = Termios::from_fd(stdin_fd)?;
    let mut raw_termios = original_termios;

    // Use cfmakeraw to set the basic raw mode
    termios::cfmakeraw(&mut raw_termios);

    // Explicitly disable echo and canonical mode (equivalent to stty -echo -icanon)
    raw_termios.c_lflag &=
        !(termios::ECHO | termios::ECHOE | termios::ECHOK | termios::ECHONL | termios::ICANON);

    // Disable signal generation
    raw_termios.c_lflag &= !termios::ISIG;

    // Disable input processing
    raw_termios.c_iflag &= !(termios::ICRNL
        | termios::INLCR
        | termios::IGNCR
        | termios::IXON
        | termios::IXOFF
        | termios::ISTRIP);

    // Disable output processing for input terminal
    raw_termios.c_oflag &= !termios::OPOST;

    // Set character size to 8 bits
    raw_termios.c_cflag &= !termios::CSIZE;
    raw_termios.c_cflag |= termios::CS8;

    // Set VMIN=1 and VTIME=0 (equivalent to stty min 1 time 0)
    raw_termios.c_cc[termios::VMIN] = 1;
    raw_termios.c_cc[termios::VTIME] = 0;

    // Apply the raw terminal settings to stdin, stdout, and stderr with TCSAFLUSH to discard any pending input
    tcsetattr(stdin_fd, termios::TCSAFLUSH, &raw_termios)?;
    tcsetattr(stdout_fd, termios::TCSAFLUSH, &raw_termios)?;
    tcsetattr(stderr_fd, termios::TCSAFLUSH, &raw_termios)?;

    Ok(original_termios)
}

fn restore_terminal(original_termios: &Termios) {
    use std::os::unix::io::AsRawFd;

    let stdin_fd = std::io::stdin().as_raw_fd();
    let stdout_fd = std::io::stdout().as_raw_fd();
    let stderr_fd = std::io::stderr().as_raw_fd();

    let _ = tcsetattr(stdin_fd, termios::TCSAFLUSH, original_termios);
    let _ = tcsetattr(stdout_fd, termios::TCSAFLUSH, original_termios);
    let _ = tcsetattr(stderr_fd, termios::TCSAFLUSH, original_termios);
}
