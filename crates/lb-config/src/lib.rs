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
    /// Protocol selector. Valid values: `"tcp"` (plain), `"tls"` (TLS
    /// 1.2/1.3 over TCP — see [`TlsConfig`]), or codec names like
    /// `"http"`, `"h2"`, `"h3"` reserved for upcoming pillars.
    pub protocol: String,
    /// TLS settings. Required when `protocol == "tls"`; must be absent
    /// otherwise.
    #[serde(default)]
    pub tls: Option<TlsConfig>,
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

/// Configuration for a single upstream backend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackendConfig {
    /// Backend address (e.g. `"127.0.0.1:3000"`).
    pub address: String,
    /// Weight for weighted load-balancing algorithms (default 1).
    #[serde(default = "default_weight")]
    pub weight: u32,
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
            "tls" => {
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
            }
            "tcp" | "http" | "h2" | "h3" => {
                if listener.tls.is_some() {
                    return Err(ConfigError::Validation(format!(
                        "listener {i} has [listeners.tls] but protocol is {protocol:?}; \
                         set protocol=\"tls\" or remove the tls block"
                    )));
                }
            }
            other => {
                return Err(ConfigError::Validation(format!(
                    "listener {i} has unknown protocol {other:?} \
                     (expected one of: tcp, tls, http, h2, h3)"
                )));
            }
        }
        for (j, backend) in listener.backends.iter().enumerate() {
            if backend.address.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "listener {i} backend {j} has an empty address"
                )));
            }
        }
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
                backends: vec![BackendConfig {
                    address: String::new(),
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
                backends: vec![],
            }],
        };
        assert!(validate_config(&config).is_err());
    }
}
