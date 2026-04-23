//! Configuration loading and management for the load balancer.
//!
//! Provides typed configuration structures and TOML parsing with validation.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![warn(clippy::pedantic, clippy::nursery)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

/// Errors from configuration parsing and validation.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// TOML deserialization failed.
    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    /// Validation error.
    #[error("validation error: {0}")]
    Validation(String),
}

/// Top-level load balancer configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LbConfig {
    /// Configured listeners.
    pub listeners: Vec<ListenerConfig>,
}

/// Configuration for a single listener.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ListenerConfig {
    /// Bind address (e.g. `"0.0.0.0:8080"`).
    pub address: String,
    /// Protocol selector. Valid values:
    ///
    /// * `"tcp"` — plain TCP proxy (default), forwarded unchanged to the
    ///   backend.
    /// * `"tls"` — TLS 1.2/1.3 over TCP with rustls. Requires
    ///   [`[listeners.tls]`](TlsConfig).
    /// * `"quic"` — QUIC over UDP with quiche. Requires
    ///   [`[listeners.quic]`](QuicListenerConfig). HTTP/3 bridging to
    ///   backends is Pillar 3b.3c-2; 3b.3c-1 validates the listener
    ///   seam + UDP binding + TLS handshake only.
    /// * `"http"`, `"h2"`, `"h3"` — reserved for upcoming pillars.
    pub protocol: String,
    /// TLS settings. Required when `protocol == "tls"`; must be absent
    /// otherwise.
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    /// QUIC settings. Required when `protocol == "quic"`; must be absent
    /// otherwise.
    #[serde(default)]
    pub quic: Option<QuicListenerConfig>,
    /// Upstream backends to load-balance across.
    #[serde(default)]
    pub backends: Vec<BackendConfig>,
}

/// TLS listener configuration (Pillar 3b.2).
///
/// Backed by rustls 0.23 + the `ring` crypto provider. The
/// [`TicketRotator`](lb-security) mints session-resumption tickets using
/// the configured rotation window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TlsConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// How often to rotate the session-ticket key (seconds). Defaults
    /// to 24 hours, matching the Step 5b default.
    #[serde(default = "default_ticket_interval")]
    pub ticket_rotation_interval_seconds: u64,
    /// Grace period during which tickets encrypted with the previous
    /// key still decrypt (seconds). Defaults to 24 hours — together
    /// with the default interval this gives a 48-hour total ticket
    /// lifetime at the rustls layer.
    #[serde(default = "default_ticket_overlap")]
    pub ticket_rotation_overlap_seconds: u64,
}

const fn default_ticket_interval() -> u64 {
    86_400
}

const fn default_ticket_overlap() -> u64 {
    86_400
}

/// QUIC listener configuration (Pillar 3b.3c-1).
///
/// Backed by quiche 0.28 + `BoringSSL`. The `retry_secret_path` stores a
/// 32-byte HMAC-SHA256 key used by
/// [`lb_security::RetryTokenSigner`](../../lb-security) for
/// stateless-retry address validation (RFC 9000 §8.1.3). The file is
/// auto-generated with mode 0600 on first boot if missing. Pillar
/// 3b.3c-2 wires the signer + replay guard to the inbound packet
/// router; 3b.3c-1 only validates the seam and the UDP bind.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuicListenerConfig {
    /// Filesystem path to the PEM-encoded certificate chain.
    pub cert_path: String,
    /// Filesystem path to the PEM-encoded private key (PKCS#8 or SEC1).
    pub key_path: String,
    /// Filesystem path to a 32-byte retry-token signing key. Auto-
    /// generated on first boot if the file does not exist.
    pub retry_secret_path: String,
    /// Connection idle timeout in milliseconds. Defaults to 30 seconds.
    #[serde(default = "default_quic_idle_timeout_ms")]
    pub max_idle_timeout_ms: u64,
    /// Maximum UDP payload the endpoint will accept. Defaults to 1350
    /// bytes (safe for a 1500-byte Ethernet MTU minus IPv4+UDP headers
    /// and QUIC overhead). Must be at least 1200 per RFC 9000 §14.
    #[serde(default = "default_quic_recv_udp_payload")]
    pub max_recv_udp_payload_size: u64,
}

const fn default_quic_idle_timeout_ms() -> u64 {
    30_000
}

const fn default_quic_recv_udp_payload() -> u64 {
    1_350
}

/// Configuration for a single upstream backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendConfig {
    /// Backend address (e.g. `"127.0.0.1:3000"`).
    pub address: String,
    /// Wire protocol spoken to this backend. Defaults to `"tcp"`.
    /// Values accepted: `"tcp"` (raw stream, used by the plain-TCP and
    /// TLS-over-TCP listeners), `"h1"` (HTTP/1.1 over TCP — the QUIC
    /// listener's default bridge target in Pillar 3b.3c-2), `"h3"`
    /// (HTTP/3 over QUIC — consumed by the Pillar 3b.3c-3 upstream
    /// pool).
    #[serde(default = "default_backend_protocol")]
    pub protocol: String,
    /// Weight for weighted load-balancing algorithms (default 1).
    #[serde(default = "default_weight")]
    pub weight: u32,
}

fn default_backend_protocol() -> String {
    "tcp".to_string()
}

const fn default_weight() -> u32 {
    1
}

/// Parse a TOML string into an `LbConfig`.
///
/// # Errors
///
/// Returns `ConfigError::TomlParse` if deserialization fails.
pub fn parse_config(input: &str) -> Result<LbConfig, ConfigError> {
    let config: LbConfig = toml::from_str(input)?;
    Ok(config)
}

/// Validate a parsed configuration.
///
/// # Errors
///
/// Returns `ConfigError::Validation` if the config is invalid.
pub fn validate_config(config: &LbConfig) -> Result<(), ConfigError> {
    if config.listeners.is_empty() {
        return Err(ConfigError::Validation(
            "at least one listener is required".into(),
        ));
    }
    for (i, listener) in config.listeners.iter().enumerate() {
        validate_listener(i, listener)?;
    }
    Ok(())
}

fn validate_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    if listener.address.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has an empty address"
        )));
    }
    let protocol = listener.protocol.trim();
    if protocol.is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has an empty protocol"
        )));
    }
    match protocol {
        "tls" => validate_tls_listener(i, listener)?,
        "quic" => validate_quic_listener(i, listener)?,
        "tcp" | "http" | "h2" | "h3" => {
            if listener.tls.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.tls] but protocol is {protocol:?}; \
                     set protocol=\"tls\" or remove the tls block"
                )));
            }
            if listener.quic.is_some() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has [listeners.quic] but protocol is {protocol:?}; \
                     set protocol=\"quic\" or remove the quic block"
                )));
            }
        }
        other => {
            return Err(ConfigError::Validation(format!(
                "listener {i} has unknown protocol {other:?} \
                 (expected one of: tcp, tls, quic, http, h2, h3)"
            )));
        }
    }
    for (j, backend) in listener.backends.iter().enumerate() {
        if backend.address.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "listener {i} backend {j} has an empty address"
            )));
        }
        match backend.protocol.as_str() {
            "tcp" | "h1" | "h3" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "listener {i} backend {j} has unknown protocol {other:?} \
                     (expected one of: tcp, h1, h3)"
                )));
            }
        }
    }
    Ok(())
}

fn validate_tls_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    let tls = listener.tls.as_ref().ok_or_else(|| {
        ConfigError::Validation(format!(
            "listener {i} has protocol=tls but is missing [listeners.tls]"
        ))
    })?;
    if tls.cert_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.cert_path is empty"
        )));
    }
    if tls.key_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.key_path is empty"
        )));
    }
    if tls.ticket_rotation_interval_seconds == 0 {
        return Err(ConfigError::Validation(format!(
            "listener {i} tls.ticket_rotation_interval_seconds must be > 0"
        )));
    }
    if listener.quic.is_some() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.quic] but protocol is \"tls\""
        )));
    }
    Ok(())
}

fn validate_quic_listener(i: usize, listener: &ListenerConfig) -> Result<(), ConfigError> {
    let quic = listener.quic.as_ref().ok_or_else(|| {
        ConfigError::Validation(format!(
            "listener {i} has protocol=quic but is missing [listeners.quic]"
        ))
    })?;
    if quic.cert_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.cert_path is empty"
        )));
    }
    if quic.key_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.key_path is empty"
        )));
    }
    if quic.retry_secret_path.trim().is_empty() {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.retry_secret_path is empty"
        )));
    }
    if quic.max_idle_timeout_ms == 0 {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.max_idle_timeout_ms must be > 0"
        )));
    }
    if quic.max_recv_udp_payload_size < 1_200 {
        return Err(ConfigError::Validation(format!(
            "listener {i} quic.max_recv_udp_payload_size must be >= 1200 (RFC 9000 §14)"
        )));
    }
    if listener.tls.is_some() {
        return Err(ConfigError::Validation(format!(
            "listener {i} has [listeners.tls] but protocol is \"quic\""
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_toml() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"
"#;
        let config = parse_config(input).unwrap();
        assert_eq!(config.listeners.len(), 1);
        assert_eq!(config.listeners[0].address, "0.0.0.0:8080");
        assert_eq!(config.listeners[0].protocol, "tcp");
    }

    #[test]
    fn parse_invalid_toml() {
        let result = parse_config("not valid toml {{{{");
        assert!(result.is_err());
    }

    #[test]
    fn validate_empty_listeners() {
        let config = LbConfig { listeners: vec![] };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_empty_address() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: String::new(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_ok() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "http".into(),
                tls: None,
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_empty_backend_address() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                backends: vec![BackendConfig {
                    address: String::new(),
                    protocol: "tcp".into(),
                    weight: 1,
                }],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_config_with_backends() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
weight = 2
"#;
        let config = parse_config(input).unwrap();
        assert_eq!(config.listeners.len(), 1);
        assert_eq!(config.listeners[0].backends.len(), 1);
        assert_eq!(config.listeners[0].backends[0].address, "127.0.0.1:3000");
        assert_eq!(config.listeners[0].backends[0].weight, 2);
    }

    #[test]
    fn parse_tls_listener() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "tls"

[listeners.tls]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"

[[listeners.backends]]
address = "127.0.0.1:3000"
"#;
        let config = parse_config(input).unwrap();
        let tls = config.listeners[0].tls.as_ref().unwrap();
        assert_eq!(tls.cert_path, "/etc/expressgateway/tls/cert.pem");
        assert_eq!(tls.key_path, "/etc/expressgateway/tls/key.pem");
        assert_eq!(tls.ticket_rotation_interval_seconds, 86_400);
        assert_eq!(tls.ticket_rotation_overlap_seconds, 86_400);
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_tls_without_block_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "tls".into(),
                tls: None,
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_unknown_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "ftp".into(),
                tls: None,
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_tls_block_without_tls_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: Some(TlsConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_tls_empty_cert_path_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "tls".into(),
                tls: Some(TlsConfig {
                    cert_path: String::new(),
                    key_path: "/y".into(),
                    ticket_rotation_interval_seconds: 86_400,
                    ticket_rotation_overlap_seconds: 86_400,
                }),
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn parse_quic_listener() {
        let input = r#"
[[listeners]]
address = "0.0.0.0:443"
protocol = "quic"

[listeners.quic]
cert_path = "/etc/expressgateway/tls/cert.pem"
key_path  = "/etc/expressgateway/tls/key.pem"
retry_secret_path = "/etc/expressgateway/quic/retry.key"

[[listeners.backends]]
address = "127.0.0.1:3000"
protocol = "h1"
"#;
        let config = parse_config(input).unwrap();
        let quic = config.listeners[0].quic.as_ref().unwrap();
        assert_eq!(quic.cert_path, "/etc/expressgateway/tls/cert.pem");
        assert_eq!(quic.max_idle_timeout_ms, 30_000);
        assert_eq!(quic.max_recv_udp_payload_size, 1_350);
        assert_eq!(config.listeners[0].backends[0].protocol, "h1");
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_quic_without_block_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: None,
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_quic_small_mtu_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:443".into(),
                protocol: "quic".into(),
                tls: None,
                quic: Some(QuicListenerConfig {
                    cert_path: "/x".into(),
                    key_path: "/y".into(),
                    retry_secret_path: "/z".into(),
                    max_idle_timeout_ms: 30_000,
                    max_recv_udp_payload_size: 500,
                }),
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_backend_unknown_protocol_rejected() {
        let config = LbConfig {
            listeners: vec![ListenerConfig {
                address: "0.0.0.0:80".into(),
                protocol: "tcp".into(),
                tls: None,
                quic: None,
                backends: vec![BackendConfig {
                    address: "127.0.0.1:3000".into(),
                    protocol: "gopher".into(),
                    weight: 1,
                }],
            }],
        };
        assert!(validate_config(&config).is_err());
    }
}
