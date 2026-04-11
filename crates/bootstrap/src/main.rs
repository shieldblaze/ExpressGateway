//! ExpressGateway binary entry point.
//!
//! Parses CLI arguments, loads configuration, builds a correctly-configured
//! Tokio runtime, and delegates to the startup sequence in [`startup::start`].

mod shutdown;
mod startup;

use std::path::PathBuf;
use std::process::ExitCode;

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

fn main() -> ExitCode {
    let args = Args::parse();
    let config_path = PathBuf::from(&args.config);

    // Step 1: Load config -- attempt the read directly, no TOCTOU exists() check.
    let mut config = match expressgateway_config::load_from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            // File might not exist or might be malformed.  If the file simply
            // doesn't exist, fall back to defaults.  For any other error (parse
            // failure, permission denied) propagate.
            if config_path.exists() {
                eprintln!("ERROR: failed to load config: {e:#}");
                return ExitCode::from(1);
            }
            // Cannot use tracing yet (subscriber not initialised), so print to
            // stderr instead.
            eprintln!(
                "WARN: config file not found: {}, using defaults",
                args.config
            );
            expressgateway_config::Config::default()
        }
    };

    // Apply CLI overrides.
    if let Some(ref level) = args.log_level {
        // Parse the CLI string into a LogLevel enum.  Accept lowercase names
        // matching the serde representation.
        let parsed: expressgateway_config::LogLevel = match level.as_str() {
            "trace" => expressgateway_config::LogLevel::Trace,
            "debug" => expressgateway_config::LogLevel::Debug,
            "info" => expressgateway_config::LogLevel::Info,
            "warn" => expressgateway_config::LogLevel::Warn,
            "error" => expressgateway_config::LogLevel::Error,
            other => {
                eprintln!("ERROR: invalid log level: {other}");
                return ExitCode::from(1);
            }
        };
        config.global.log_level = parsed;
    }

    // Step 2: If --validate, just check and exit.
    if args.validate {
        match expressgateway_config::validate(&config) {
            Ok(()) => {
                println!("Configuration is valid.");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("Configuration invalid: {e:#}");
                return ExitCode::from(1);
            }
        }
    }

    // Step 3: Build the Tokio runtime from config before entering async.
    let worker_threads = if config.runtime.worker_threads == 0 {
        num_cpus()
    } else {
        config.runtime.worker_threads
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(worker_threads)
        .thread_name("eg-worker")
        .thread_stack_size(4 * 1024 * 1024) // 4 MiB -- generous for deep stacks
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("ERROR: failed to build Tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    // Step 4: Determine the config path to pass for hot-reload.
    let reload_path = if config_path.is_file() {
        Some(config_path)
    } else {
        None
    };

    // Step 5: Run the startup sequence on the configured runtime.
    match runtime.block_on(startup::start(config, reload_path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e:#}");
            ExitCode::from(1)
        }
    }
}

/// Return the number of available CPU cores.
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}
