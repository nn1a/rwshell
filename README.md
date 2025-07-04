# rwshell

A Rust-based program for sharing your terminal over the web.

## Features

- Real-time terminal sharing via WebSocket
- Read-only mode support
- Access terminal from a web browser
- Responsive web interface

## Installation & Usage

### Build

```bash
cargo build --release
```

### Run Server

```bash
# Default (localhost:8000)
cargo run

# Run on another address
cargo run -- --listen 0.0.0.0:3000

# Run in read-only mode
cargo run -- --readonly
```

## Options

- `--command`: Command to run (default: system default shell)
- `--args`: Command arguments
- `--listen`: Server address (default: localhost:8000)
- `--readonly`: Read-only mode
- `--headless`: Headless mode
- `--verbose`: Verbose logging
- `--version`: Show version info
- `--uuid`: Set a custom session UUID

## How to Use

1. Start the server
2. Open your web browser and go to `http://localhost:8000/s/local/`
3. The terminal will appear in your browser
