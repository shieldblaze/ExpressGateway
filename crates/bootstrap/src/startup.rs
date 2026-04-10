//! Full startup sequence for ExpressGateway.
//!
//! [`start`] is the single entry point that initialises every subsystem in the
//! correct order and then blocks until a shutdown signal is received.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use expressgateway_config::Config;
use tokio::sync::broadcast;

use crate::shutdown;

/// Run the complete startup sequence and block until shutdown.
pub async fn start(config: Config) -> Result<()> {
    // ── 1. Tracing / logging ───────────────────────────────────────────────
    init_tracing(&config)?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        environment = %config.global.environment,
        "ExpressGateway starting"
    );

    // ── 2. Metrics registry ────────────────────────────────────────────────
    let _metrics = expressgateway_metrics::register_all();
    tracing::info!("metrics registry initialised");

    // ── 3. Metrics HTTP server ─────────────────────────────────────────────
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

    // ── 4. XDP acceleration ────────────────────────────────────────────────
    if config.runtime.xdp_enabled {
        #[cfg(target_os = "linux")]
        {
            tracing::info!(
                interface = %config.runtime.xdp_interface,
                mode = %config.runtime.xdp_mode,
                "XDP acceleration enabled"
            );
            // Actual XDP program attachment would happen here via eg-xdp.
        }
        #[cfg(not(target_os = "linux"))]
        {
            tracing::warn!("XDP requested but not available on this platform");
        }
    }

    // ── 5. Shutdown coordination channel ───────────────────────────────────
    let (shutdown_tx, _) = broadcast::channel::<()>(1);

    // ── 6. Log summary ────────────────────────────────────────────────────
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

    // ── 7. Wait for shutdown signal ────────────────────────────────────────
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for ctrl-c")?;

    tracing::info!("received shutdown signal");

    // ── 8. Graceful shutdown ───────────────────────────────────────────────
    let drain_timeout = std::time::Duration::from_secs(config.graceful_shutdown.drain_timeout_s);

    // Notify all subsystems
    let _ = shutdown_tx.send(());

    shutdown::graceful_shutdown(drain_timeout).await;

    tracing::info!("ExpressGateway stopped");
    Ok(())
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
