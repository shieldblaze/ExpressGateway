//! ExpressGateway binary entry point.
//!
//! Parses CLI arguments, loads configuration, and delegates to the startup
//! sequence in [`startup::start`].

mod shutdown;
mod startup;

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "expressgateway",
    version,
    about = "ExpressGateway - High-Performance L4/L7 Load Balancer"
)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "/etc/expressgateway/config.toml")]
    config: String,

    /// Override log level
    #[arg(short, long)]
    log_level: Option<String>,

    /// Validate config and exit
    #[arg(long)]
    validate: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config_path = PathBuf::from(&args.config);

    // Step 1: Load config
    let mut config = if config_path.exists() {
        expressgateway_config::load_from_file(&config_path)?
    } else {
        // Cannot use tracing yet (subscriber not initialised), so print to
        // stderr instead.
        eprintln!(
            "WARN: config file not found: {}, using defaults",
            args.config
        );
        expressgateway_config::Config::default()
    };

    // Apply CLI overrides
    if let Some(ref level) = args.log_level {
        config.global.log_level = level.clone();
    }

    // Step 2: If --validate, just check and exit
    if args.validate {
        expressgateway_config::validate(&config)?;
        println!("Configuration is valid.");
        return Ok(());
    }

    // Step 3: Hand off to the full startup sequence
    let path = if config_path.exists() {
        Some(config_path)
    } else {
        None
    };
    startup::start(config, path).await
}
