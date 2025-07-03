use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "rwshell")]
#[command(about = "Share your terminal over the web")]
pub struct Args {
    /// [s] The command to run
    #[arg(long, default_value_t = get_default_shell())]
    pub command: String,

    /// [s] The command arguments
    #[arg(long, default_value = "")]
    pub args: String,

    /// [s] rwshell server address
    #[arg(long, default_value = "localhost:8000")]
    pub listen: String,

    /// Print the rwshell version
    #[arg(long)]
    pub version: bool,

    /// [s] Start a read only session
    #[arg(long)]
    pub readonly: bool,

    /// [s] Don't expect an interactive terminal at stdin
    #[arg(long)]
    pub headless: bool,

    /// [s] Number of cols for the allocated pty when running headless
    #[arg(long, default_value = "80")]
    pub headless_cols: u16,

    /// [s] Number of rows for the allocated pty when running headless
    #[arg(long, default_value = "25")]
    pub headless_rows: u16,

    /// Generate a random UUID for the session URL
    #[arg(long)]
    pub uuid: bool,

    /// Verbose logging
    #[arg(long)]
    pub verbose: bool,
}

fn get_default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string())
}
