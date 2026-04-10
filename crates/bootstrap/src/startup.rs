//! Full startup sequence for ExpressGateway.
//!
//! [`start`] is the single entry point that initialises every subsystem in the
//! correct order and then blocks until a shutdown signal is received.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use expressgateway_config::{Config, ConfigWatcher};
use tokio::sync::broadcast;

use crate::shutdown;

/// Run the complete startup sequence and block until shutdown.
pub async fn start(config: Config, config_path: Option<PathBuf>) -> Result<()> {
    // -- 1. Tracing / logging --------------------------------------------------
    init_tracing(&config)?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = %config.global.environment,
        "ExpressGateway starting"
    );

    // -- 2. Metrics registry ---------------------------------------------------
    let _metrics = expressgateway_metrics::register_all();
    tracing::info!("metrics registry initialised");

    // -- 3. Metrics HTTP server ------------------------------------------------
    let metrics_addr: SocketAddr = config
        .global
        .metrics_bind
        .parse()
        .context("invalid metrics_bind address")?;

    tokio::spawn(async move {
        if let Err(e) = expressgateway_metrics::serve_metrics(metrics_addr).await {
            tracing::error!(error = %e, "metrics server failed");
        }
    });
    tracing::info!(bind = %metrics_addr, "metrics server started");

    // -- 4. Config watcher (hot-reload) ----------------------------------------
    let config_watcher = if let Some(ref path) = config_path {
        let watcher = Arc::new(ConfigWatcher::from_config(config.clone(), path.clone()));
        match watcher.spawn_watchers() {
            Ok(shutdown_handle) => {
                tracing::info!(path = %path.display(), "config hot-reload watcher started");
                // Keep the shutdown handle alive so the watcher tasks persist.
                // It will be dropped during shutdown.
                Some((watcher, shutdown_handle))
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to start config watcher, hot-reload disabled");
                None
            }
        }
    } else {
        None
    };

    // -- 5. XDP acceleration ---------------------------------------------------
    if config.runtime.xdp_enabled {
        #[cfg(target_os = "linux")]
        {
            tracing::info!(
                interface = %config.runtime.xdp_interface,
                mode = %config.runtime.xdp_mode,
                "XDP acceleration enabled"
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("XDP requested but not available on this platform");
        }
    }

    // -- 6. Shutdown coordination channel --------------------------------------
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // -- 7. Log summary --------------------------------------------------------
    tracing::info!(
        listeners = config.listeners.len(),
        clusters = config.clusters.len(),
        routes = config.routes.len(),
        "ExpressGateway started successfully"
    );

    for listener in &config.listeners {
        tracing::info!(
            name = %listener.name,
            protocol = %listener.protocol,
            bind = %listener.bind,
            io_backend = ?listener.io_backend,
            "listener configured"
        );
    }

    for cluster in &config.clusters {
        tracing::info!(
            name = %cluster.name,
            strategy = %cluster.lb_strategy,
            nodes = cluster.nodes.len(),
            "cluster configured"
        );
    }

    if config.controlplane.enabled {
        tracing::info!(
            grpc = %config.controlplane.grpc_bind,
            rest = %config.controlplane.rest_bind,
            "control plane enabled"
        );
    }

    // -- 8. Wait for shutdown signal -------------------------------------------
    wait_for_shutdown_signal().await;

    tracing::info!("received shutdown signal");

    // -- 9. Graceful shutdown --------------------------------------------------
    let drain_timeout = std::time::Duration::from_secs(config.graceful_shutdown.drain_timeout_s);

    // Drop the config watcher to stop file-watching tasks.
    drop(config_watcher);

    shutdown::graceful_shutdown(drain_timeout, shutdown_tx).await;

    tracing::info!("ExpressGateway stopped");
    Ok(())
}

/// Wait for a termination signal (SIGTERM, SIGINT, or Ctrl-C).
///
/// On Unix this listens for both SIGTERM and SIGINT.  On non-Unix it falls
/// back to `tokio::signal::ctrl_c()`.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt())
            .expect("failed to register SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM");
            }
            _ = sigint.recv() => {
                tracing::info!("received SIGINT");
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for ctrl-c");
    }
}

/// Initialise the `tracing` subscriber with JSON formatting and an
/// environment-based filter.
fn init_tracing(config: &Config) -> Result<()> {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.global.log_level));

    tracing_subscriber::registry()
        .with(fmt::layer().json())
        .with(filter)
        .init();

    Ok(())
}
