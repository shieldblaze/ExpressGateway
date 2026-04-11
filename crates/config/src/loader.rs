//! Configuration loading and validation.
//!
//! The [`load_from_file`] function reads a TOML file, deserialises it into
//! [`Config`], and runs full validation before returning.  Call [`validate`]
//! directly if you already have an in-memory `Config`.
//!
//! [`load_from_file_async`] performs the same work on a blocking thread pool
//! so it is safe to call from an async context.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::model::Config;

/// Load a [`Config`] from a TOML file, validating it before returning.
///
/// This performs blocking I/O.  When calling from an async context, prefer
/// [`load_from_file_async`].
///
/// # Errors
///
/// Returns an error when the file cannot be read, the TOML is malformed, or
/// validation fails.
pub fn load_from_file(path: &Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("failed to parse TOML from: {}", path.display()))?;
    validate(&config)?;
    Ok(config)
}

/// Async-safe variant of [`load_from_file`].
///
/// Runs the blocking file read and parse on `tokio::task::spawn_blocking`
/// so it does not block a Tokio worker thread.
pub async fn load_from_file_async(path: &Path) -> Result<Config> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || load_from_file(&path))
        .await
        .context("config load task panicked")?
}

/// Load a [`Config`] from a TOML string, validating it before returning.
///
/// This is primarily useful for tests and embedded configs.
pub fn load_from_str(toml_str: &str) -> Result<Config> {
    let config: Config = toml::from_str(toml_str).context("failed to parse TOML from string")?;
    validate(&config)?;
    Ok(config)
}

/// Validate every field in the configuration.
///
/// Enum-typed fields are validated at parse time by serde; this function
/// checks semantic constraints:
/// - All bind addresses are parseable as `SocketAddr`
/// - Routes reference clusters that exist
/// - Node weights are positive
/// - Timeouts are positive
/// - No duplicate listener names or bind addresses
/// - No duplicate cluster names
/// - When TLS is enabled, cert/key paths are set
/// - HTTP/2 window size within spec bounds
/// - Compression levels within algorithm bounds
/// - Retry backoff values are sane
pub fn validate(config: &Config) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // -- Global ---------------------------------------------------------------
    validate_bind_addr(
        &config.global.metrics_bind,
        "global.metrics_bind",
        &mut errors,
    );

    // -- Transport ------------------------------------------------------------
    if config.transport.connect_timeout_ms == 0 {
        errors.push("transport.connect_timeout_ms must be positive".into());
    }

    // -- TLS ------------------------------------------------------------------
    if config.tls.handshake_timeout_ms == 0 {
        errors.push("tls.handshake_timeout_ms must be positive".into());
    }

    if config.tls.session_timeout_s == 0 {
        errors.push("tls.session_timeout_s must be positive".into());
    }

    if config.tls.enabled {
        if config.tls.server.default_cert.is_empty() {
            errors.push(
                "tls.server.default_cert must be set when tls.enabled is true".into(),
            );
        }
        if config.tls.server.default_key.is_empty() {
            errors.push(
                "tls.server.default_key must be set when tls.enabled is true".into(),
            );
        }
    }

    for (i, sni) in config.tls.sni_certs.iter().enumerate() {
        let label = format!("tls.sni_certs[{i}]");
        if sni.hostname.is_empty() {
            errors.push(format!("{label}.hostname must not be empty"));
        }
        if sni.cert.is_empty() {
            errors.push(format!("{label}.cert must not be empty"));
        }
        if sni.key.is_empty() {
            errors.push(format!("{label}.key must not be empty"));
        }
    }

    // -- Listeners (bind addresses + uniqueness) ------------------------------
    let mut seen_binds: HashSet<String> = HashSet::new();
    let mut seen_listener_names: HashSet<String> = HashSet::new();
    for (i, listener) in config.listeners.iter().enumerate() {
        let label = format!("listeners[{i}]");

        if listener.name.is_empty() {
            errors.push(format!("{label}.name must not be empty"));
        } else if !seen_listener_names.insert(listener.name.clone()) {
            errors.push(format!(
                "{label}.name \"{}\" is a duplicate listener name",
                listener.name
            ));
        }

        validate_bind_addr(&listener.bind, &format!("{label}.bind"), &mut errors);

        if !seen_binds.insert(listener.bind.clone()) {
            errors.push(format!(
                "{label}.bind \"{}\" is a duplicate listener address",
                listener.bind
            ));
        }
    }

    // -- Clusters -------------------------------------------------------------
    let mut seen_cluster_names: HashSet<&str> = HashSet::new();
    for (i, cluster) in config.clusters.iter().enumerate() {
        let label = format!("clusters[{i}]");

        if cluster.name.is_empty() {
            errors.push(format!("{label}.name must not be empty"));
        } else if !seen_cluster_names.insert(&cluster.name) {
            errors.push(format!(
                "{label}.name \"{}\" is a duplicate cluster name",
                cluster.name
            ));
        }

        if cluster.nodes.is_empty() {
            errors.push(format!("{label}.nodes must not be empty"));
        }

        for (j, node) in cluster.nodes.iter().enumerate() {
            let nlabel = format!("{label}.nodes[{j}]");

            validate_bind_addr(&node.address, &format!("{nlabel}.address"), &mut errors);

            if let Some(w) = node.weight
                && w == 0
            {
                errors.push(format!("{nlabel}.weight must be >= 1"));
            }
        }
    }

    // -- Routes (cluster references) ------------------------------------------
    let cluster_names: HashSet<&str> = config.clusters.iter().map(|c| c.name.as_str()).collect();
    for (i, route) in config.routes.iter().enumerate() {
        let label = format!("routes[{i}]");

        if route.cluster.is_empty() {
            errors.push(format!("{label}.cluster must not be empty"));
        } else if !cluster_names.contains(route.cluster.as_str()) {
            errors.push(format!(
                "{label}.cluster \"{}\" does not reference an existing cluster",
                route.cluster
            ));
        }
    }

    // -- HTTP -----------------------------------------------------------------
    if config.http.request_header_timeout_ms == 0 {
        errors.push("http.request_header_timeout_ms must be positive".into());
    }
    if config.http.request_body_timeout_ms == 0 {
        errors.push("http.request_body_timeout_ms must be positive".into());
    }
    if config.http.idle_timeout_s == 0 {
        errors.push("http.idle_timeout_s must be positive".into());
    }
    if config.http.keepalive_timeout_s == 0 {
        errors.push("http.keepalive_timeout_s must be positive".into());
    }

    // HTTP/2 spec: initial window size is 65535..=2^31-1
    let h2_win = config.http.h2_connection_window_size;
    if !(65_535..=2_147_483_647).contains(&h2_win) {
        errors.push(format!(
            "http.h2_connection_window_size must be between 65535 and 2147483647, got {h2_win}"
        ));
    }

    // -- Compression levels ---------------------------------------------------
    for (algo, level) in &config.http.compression.levels {
        let max = match algo {
            crate::model::CompressionAlgorithm::Gzip => 9,
            crate::model::CompressionAlgorithm::Brotli => 11,
            crate::model::CompressionAlgorithm::Zstd => 22,
        };
        if *level == 0 || *level > max {
            errors.push(format!(
                "http.compression.levels.{algo}: level must be 1..={max}, got {level}"
            ));
        }
    }

    // -- Health Check ---------------------------------------------------------
    if config.health_check.interval_s == 0 {
        errors.push("health_check.interval_s must be positive".into());
    }
    if config.health_check.timeout_ms == 0 {
        errors.push("health_check.timeout_ms must be positive".into());
    }
    if config.health_check.rise == 0 {
        errors.push("health_check.rise must be positive".into());
    }
    if config.health_check.fall == 0 {
        errors.push("health_check.fall must be positive".into());
    }

    // -- Control Plane --------------------------------------------------------
    if config.controlplane.enabled {
        validate_bind_addr(
            &config.controlplane.grpc_bind,
            "controlplane.grpc_bind",
            &mut errors,
        );
        validate_bind_addr(
            &config.controlplane.rest_bind,
            "controlplane.rest_bind",
            &mut errors,
        );
        if config.controlplane.heartbeat_interval_s == 0 {
            errors.push("controlplane.heartbeat_interval_s must be positive".into());
        }
    }

    // -- Graceful Shutdown ----------------------------------------------------
    if config.graceful_shutdown.drain_timeout_s == 0 {
        errors.push("graceful_shutdown.drain_timeout_s must be positive".into());
    }

    // -- Retry ----------------------------------------------------------------
    if config.retry.enabled {
        if config.retry.max_retries == 0 {
            errors.push("retry.max_retries must be positive when retries are enabled".into());
        }
        if config.retry.initial_backoff_ms == 0 {
            errors.push("retry.initial_backoff_ms must be positive".into());
        }
        if config.retry.max_backoff_ms < config.retry.initial_backoff_ms {
            errors.push(
                "retry.max_backoff_ms must be >= retry.initial_backoff_ms".into(),
            );
        }
        if config.retry.backoff_multiplier == 0 {
            errors.push("retry.backoff_multiplier must be positive".into());
        }
    }

    // -- Report ---------------------------------------------------------------
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(
            "configuration validation failed with {} error(s):\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn validate_bind_addr(addr: &str, field: &str, errors: &mut Vec<String>) {
    if addr.parse::<SocketAddr>().is_err() {
        errors.push(format!(
            "{field} \"{addr}\" is not a valid socket address (expected host:port)",
        ));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Config;
    use std::io::Write;

    #[test]
    fn default_config_validates() {
        let cfg = Config::default();
        validate(&cfg).expect("default config should be valid");
    }

    #[test]
    fn invalid_environment_rejected_by_serde() {
        let toml = r#"
[global]
environment = "staging"
"#;
        let result: Result<Config, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_bind_address_rejected() {
        let mut cfg = Config::default();
        cfg.global.metrics_bind = "not-an-addr".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("global.metrics_bind"));
    }

    #[test]
    fn duplicate_listener_bind_rejected() {
        let mut cfg = Config::default();
        cfg.listeners = vec![
            crate::model::ListenerConfig {
                name: "a".into(),
                bind: "0.0.0.0:8080".into(),
                ..Default::default()
            },
            crate::model::ListenerConfig {
                name: "b".into(),
                bind: "0.0.0.0:8080".into(),
                ..Default::default()
            },
        ];
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn duplicate_listener_name_rejected() {
        let mut cfg = Config::default();
        cfg.listeners = vec![
            crate::model::ListenerConfig {
                name: "web".into(),
                bind: "0.0.0.0:8080".into(),
                ..Default::default()
            },
            crate::model::ListenerConfig {
                name: "web".into(),
                bind: "0.0.0.0:8081".into(),
                ..Default::default()
            },
        ];
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("duplicate listener name"));
    }

    #[test]
    fn duplicate_cluster_name_rejected() {
        let mut cfg = Config::default();
        cfg.clusters = vec![
            crate::model::ClusterConfig {
                name: "backend".into(),
                ..Default::default()
            },
            crate::model::ClusterConfig {
                name: "backend".into(),
                ..Default::default()
            },
        ];
        // Routes also reference "default" which doesn't exist, so fix that.
        cfg.routes[0].cluster = "backend".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("duplicate cluster name"));
    }

    #[test]
    fn route_referencing_missing_cluster_rejected() {
        let mut cfg = Config::default();
        cfg.routes[0].cluster = "nonexistent".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn zero_weight_rejected() {
        let mut cfg = Config::default();
        cfg.clusters[0].nodes[0].weight = Some(0);
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("weight"));
    }

    #[test]
    fn zero_timeout_rejected() {
        let mut cfg = Config::default();
        cfg.transport.connect_timeout_ms = 0;
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("connect_timeout_ms"));
    }

    #[test]
    fn invalid_lb_strategy_rejected_by_serde() {
        let toml = r#"
[[clusters]]
name = "x"
lb_strategy = "magic"
"#;
        let result: Result<Config, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn load_from_str_works() {
        let toml = r#"
[global]
environment = "development"
log_level = "debug"
metrics_bind = "127.0.0.1:9090"
"#;
        let cfg = load_from_str(toml).expect("valid toml");
        assert_eq!(cfg.global.environment, crate::model::Environment::Development);
    }

    #[test]
    fn load_from_file_works() {
        let dir = std::env::temp_dir().join("expressgateway_config_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            r#"
[global]
environment = "development"
log_level = "debug"
metrics_bind = "127.0.0.1:9090"
"#
        )
        .unwrap();

        let cfg = load_from_file(&path).expect("load from file");
        assert_eq!(cfg.global.environment, crate::model::Environment::Development);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_proxy_protocol_rejected_by_serde() {
        let toml = r#"
[proxy_protocol]
outbound = "auto"
"#;
        let result: Result<Config, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn empty_cluster_nodes_rejected() {
        let mut cfg = Config::default();
        cfg.clusters[0].nodes.clear();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("nodes must not be empty"));
    }

    #[test]
    fn multiple_errors_collected() {
        let mut cfg = Config::default();
        cfg.global.metrics_bind = "bad".into();
        cfg.transport.connect_timeout_ms = 0;
        cfg.graceful_shutdown.drain_timeout_s = 0;
        let err = validate(&cfg).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("3 error(s)"));
    }

    #[test]
    fn tls_enabled_without_certs_rejected() {
        let mut cfg = Config::default();
        cfg.tls.enabled = true;
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("default_cert"));
        assert!(err.to_string().contains("default_key"));
    }

    #[test]
    fn h2_window_too_small_rejected() {
        let mut cfg = Config::default();
        cfg.http.h2_connection_window_size = 100;
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("h2_connection_window_size"));
    }

    #[test]
    fn compression_level_out_of_range_rejected() {
        let mut cfg = Config::default();
        cfg.http.compression.levels.insert(
            crate::model::CompressionAlgorithm::Gzip,
            99,
        );
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("gzip"));
    }

    #[test]
    fn retry_max_backoff_less_than_initial_rejected() {
        let mut cfg = Config::default();
        cfg.retry.initial_backoff_ms = 5000;
        cfg.retry.max_backoff_ms = 100;
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("max_backoff_ms"));
    }
}