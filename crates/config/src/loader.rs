//! Configuration loading and validation.
//!
//! The [`load_from_file`] function reads a TOML file, deserialises it into
//! [`Config`], and runs full validation before returning.  Call [`validate`]
//! directly if you already have an in-memory `Config`.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::model::Config;

/// Load a [`Config`] from a TOML file, validating it before returning.
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
/// Rules enforced:
/// - All bind addresses must be parseable as `SocketAddr`
/// - Ports are in the 1..=65535 range (implied by `SocketAddr` parsing)
/// - Routes reference clusters that exist
/// - Node weights are positive
/// - Timeouts are positive
/// - No duplicate listener bind addresses
/// - Environment is either `"production"` or `"development"`
/// - Log level is one of the standard tracing levels
/// - TLS profile is either `"modern"` or `"intermediate"`
/// - Proxy-protocol modes are valid
/// - Security mode is `"allowlist"` or `"denylist"`
/// - Mutual TLS mode is valid
/// - Runtime backend is `"auto"`, `"io_uring"`, or `"epoll"`
/// - Load-balancing strategy is recognised
pub fn validate(config: &Config) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // -- Global ---------------------------------------------------------------
    if !["production", "development"].contains(&config.global.environment.as_str()) {
        errors.push(format!(
            "global.environment must be \"production\" or \"development\", got \"{}\"",
            config.global.environment
        ));
    }

    if !["trace", "debug", "info", "warn", "error"].contains(&config.global.log_level.as_str()) {
        errors.push(format!(
            "global.log_level must be one of trace/debug/info/warn/error, got \"{}\"",
            config.global.log_level
        ));
    }

    validate_bind_addr(
        &config.global.metrics_bind,
        "global.metrics_bind",
        &mut errors,
    );

    // -- Runtime --------------------------------------------------------------
    if !["auto", "io_uring", "epoll"].contains(&config.runtime.backend.as_str()) {
        errors.push(format!(
            "runtime.backend must be auto/io_uring/epoll, got \"{}\"",
            config.runtime.backend
        ));
    }

    if config.runtime.xdp_enabled && config.runtime.xdp_interface.is_empty() {
        errors.push("runtime.xdp_interface must be set when xdp_enabled is true".into());
    }

    if !["driver", "generic"].contains(&config.runtime.xdp_mode.as_str()) {
        errors.push(format!(
            "runtime.xdp_mode must be driver/generic, got \"{}\"",
            config.runtime.xdp_mode
        ));
    }

    // -- Transport ------------------------------------------------------------
    if config.transport.connect_timeout_ms == 0 {
        errors.push("transport.connect_timeout_ms must be positive".into());
    }

    // -- TLS ------------------------------------------------------------------
    if !["modern", "intermediate"].contains(&config.tls.profile.as_str()) {
        errors.push(format!(
            "tls.profile must be modern/intermediate, got \"{}\"",
            config.tls.profile
        ));
    }

    if config.tls.handshake_timeout_ms == 0 {
        errors.push("tls.handshake_timeout_ms must be positive".into());
    }

    if config.tls.session_timeout_s == 0 {
        errors.push("tls.session_timeout_s must be positive".into());
    }

    if !["not_required", "optional", "required"].contains(&config.tls.server.mutual_tls.as_str()) {
        errors.push(format!(
            "tls.server.mutual_tls must be not_required/optional/required, got \"{}\"",
            config.tls.server.mutual_tls
        ));
    }

    // -- Listeners (bind addresses + uniqueness) ------------------------------
    let mut seen_binds: HashSet<String> = HashSet::new();
    for (i, listener) in config.listeners.iter().enumerate() {
        let label = format!("listeners[{}]", i);

        if listener.name.is_empty() {
            errors.push(format!("{label}.name must not be empty"));
        }

        validate_bind_addr(&listener.bind, &format!("{label}.bind"), &mut errors);

        if !seen_binds.insert(listener.bind.clone()) {
            errors.push(format!(
                "{label}.bind \"{}\" is a duplicate listener address",
                listener.bind
            ));
        }

        if !["http", "https", "tcp", "udp"].contains(&listener.protocol.as_str()) {
            errors.push(format!(
                "{label}.protocol must be http/https/tcp/udp, got \"{}\"",
                listener.protocol
            ));
        }

        if let Some(ref backend) = listener.io_backend
            && !["auto", "io_uring", "epoll"].contains(&backend.as_str())
        {
            errors.push(format!(
                "{label}.io_backend must be auto/io_uring/epoll, got \"{backend}\""
            ));
        }
    }

    // -- Clusters -------------------------------------------------------------
    let cluster_names: HashSet<&str> = config.clusters.iter().map(|c| c.name.as_str()).collect();

    for (i, cluster) in config.clusters.iter().enumerate() {
        let label = format!("clusters[{}]", i);

        if cluster.name.is_empty() {
            errors.push(format!("{label}.name must not be empty"));
        }

        validate_lb_strategy(&cluster.lb_strategy, &label, &mut errors);

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
    for (i, route) in config.routes.iter().enumerate() {
        let label = format!("routes[{}]", i);

        if route.cluster.is_empty() {
            errors.push(format!("{label}.cluster must not be empty"));
        } else if !cluster_names.contains(route.cluster.as_str()) {
            errors.push(format!(
                "{label}.cluster \"{}\" does not reference an existing cluster",
                route.cluster
            ));
        }

        if let Some(ref strategy) = route.lb_strategy {
            validate_lb_strategy(strategy, &label, &mut errors);
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

    // -- Proxy Protocol -------------------------------------------------------
    if !["off", "v1", "v2", "auto"].contains(&config.proxy_protocol.inbound.as_str()) {
        errors.push(format!(
            "proxy_protocol.inbound must be off/v1/v2/auto, got \"{}\"",
            config.proxy_protocol.inbound
        ));
    }
    if !["off", "v1", "v2"].contains(&config.proxy_protocol.outbound.as_str()) {
        errors.push(format!(
            "proxy_protocol.outbound must be off/v1/v2, got \"{}\"",
            config.proxy_protocol.outbound
        ));
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

    // -- Security -------------------------------------------------------------
    if !["allowlist", "denylist"].contains(&config.security.mode.as_str()) {
        errors.push(format!(
            "security.mode must be allowlist/denylist, got \"{}\"",
            config.security.mode
        ));
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
            "{field} \"{}\" is not a valid socket address (expected host:port)",
            addr
        ));
    }
}

fn validate_lb_strategy(strategy: &str, label: &str, errors: &mut Vec<String>) {
    const VALID: &[&str] = &[
        "round_robin",
        "least_connections",
        "ip_hash",
        "random",
        "weighted",
    ];
    if !VALID.contains(&strategy) {
        errors.push(format!(
            "{label}.lb_strategy must be one of {VALID:?}, got \"{strategy}\""
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
    fn invalid_environment_rejected() {
        let mut cfg = Config::default();
        cfg.global.environment = "staging".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("global.environment"));
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
    fn invalid_lb_strategy_rejected() {
        let mut cfg = Config::default();
        cfg.clusters[0].lb_strategy = "magic".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("lb_strategy"));
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
        assert_eq!(cfg.global.environment, "development");
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
        assert_eq!(cfg.global.environment, "development");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_security_mode_rejected() {
        let mut cfg = Config::default();
        cfg.security.mode = "permissive".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("security.mode"));
    }

    #[test]
    fn invalid_proxy_protocol_rejected() {
        let mut cfg = Config::default();
        cfg.proxy_protocol.outbound = "auto".into();
        let err = validate(&cfg).unwrap_err();
        assert!(err.to_string().contains("proxy_protocol.outbound"));
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
        cfg.global.environment = "staging".into();
        cfg.global.log_level = "verbose".into();
        cfg.global.metrics_bind = "bad".into();
        let err = validate(&cfg).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("3 error(s)"));
    }
}
