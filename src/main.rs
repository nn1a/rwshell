use anyhow::Result;
use clap::Parser;
use tracing::debug;

mod args;
mod assets;
mod server;

use args::Args;
use server::RwShellServer;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };

    tracing_subscriber::fmt()
        .with_env_filter(format!("rwshell={log_level}"))
        .init();

    // Print version if requested
    if args.version {
        println!("{VERSION}");
        return Ok(());
    }

    // Check if stdin is a terminal (unless running headless)
    if !args.headless && !atty::is(atty::Stream::Stdin) {
        eprintln!("Input not a tty");
        std::process::exit(1);
    }

    // Server mode - start a new sharing session
    debug!("Starting rwshell server");

    let server = RwShellServer::new(args).await?;
    server.run().await?;

    println!("rwshell finished");
    Ok(())
}
