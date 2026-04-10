//! JSON structured access logging.
//!
//! Each proxied request produces an [`AccessLogEntry`] that is serialised to
//! JSON and emitted through the `tracing` infrastructure.

use serde::{Deserialize, Serialize};

/// A single access-log record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// HTTP method (e.g. `GET`, `POST`).
    pub method: String,
    /// Request URI / path.
    pub uri: String,
    /// Response status code.
    pub status: u16,
    /// Request latency in milliseconds.
    pub latency_ms: f64,
    /// Bytes sent to the client.
    pub bytes_sent: u64,
    /// Bytes received from the client.
    pub bytes_received: u64,
    /// Unique request identifier.
    pub request_id: String,
    /// Protocol used (e.g. `http`, `https`, `h2c`).
    pub protocol: String,
    /// TLS version if applicable.
    pub tls_version: Option<String>,
    /// TLS cipher suite if applicable.
    pub tls_cipher: Option<String>,
    /// Client IP address.
    pub client_ip: String,
    /// Backend that served the request.
    pub backend: String,
    /// User-Agent header value.
    pub user_agent: Option<String>,
}

impl AccessLogEntry {
    /// Serialise this entry to a JSON string.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

/// Simple access logger that emits entries via [`tracing`].
pub struct AccessLogger;

impl AccessLogger {
    /// Create a new `AccessLogger`.
    pub fn new() -> Self {
        Self
    }

    /// Log an access-log entry at `INFO` level.
    pub fn log(&self, entry: &AccessLogEntry) {
        match entry.to_json() {
            Ok(json) => tracing::info!(target: "access_log", "{}", json),
            Err(err) => {
                tracing::error!(target: "access_log", "failed to serialise access log: {}", err)
            }
        }
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> AccessLogEntry {
        AccessLogEntry {
            timestamp: "2026-04-10T12:00:00Z".to_string(),
            method: "GET".to_string(),
            uri: "/api/v1/health".to_string(),
            status: 200,
            latency_ms: 1.234,
            bytes_sent: 512,
            bytes_received: 128,
            request_id: "req-abc-123".to_string(),
            protocol: "https".to_string(),
            tls_version: Some("TLSv1.3".to_string()),
            tls_cipher: Some("TLS_AES_256_GCM_SHA384".to_string()),
            client_ip: "192.168.1.42".to_string(),
            backend: "backend-1".to_string(),
            user_agent: Some("curl/8.0".to_string()),
        }
    }

    #[test]
    fn test_to_json_roundtrip() {
        let entry = sample_entry();
        let json = entry.to_json().expect("serialisation should succeed");
        let parsed: AccessLogEntry =
            serde_json::from_str(&json).expect("deserialisation should succeed");

        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.uri, "/api/v1/health");
        assert_eq!(parsed.status, 200);
        assert!((parsed.latency_ms - 1.234).abs() < f64::EPSILON);
        assert_eq!(parsed.bytes_sent, 512);
        assert_eq!(parsed.bytes_received, 128);
        assert_eq!(parsed.request_id, "req-abc-123");
        assert_eq!(parsed.protocol, "https");
        assert_eq!(parsed.tls_version.as_deref(), Some("TLSv1.3"));
        assert_eq!(parsed.tls_cipher.as_deref(), Some("TLS_AES_256_GCM_SHA384"));
        assert_eq!(parsed.client_ip, "192.168.1.42");
        assert_eq!(parsed.backend, "backend-1");
        assert_eq!(parsed.user_agent.as_deref(), Some("curl/8.0"));
    }

    #[test]
    fn test_to_json_contains_fields() {
        let entry = sample_entry();
        let json = entry.to_json().unwrap();

        assert!(json.contains("\"method\":\"GET\""));
        assert!(json.contains("\"status\":200"));
        assert!(json.contains("\"latency_ms\":1.234"));
        assert!(json.contains("\"tls_version\":\"TLSv1.3\""));
        assert!(json.contains("\"client_ip\":\"192.168.1.42\""));
    }

    #[test]
    fn test_to_json_optional_none() {
        let mut entry = sample_entry();
        entry.tls_version = None;
        entry.tls_cipher = None;
        entry.user_agent = None;

        let json = entry.to_json().unwrap();
        assert!(json.contains("\"tls_version\":null"));
        assert!(json.contains("\"tls_cipher\":null"));
        assert!(json.contains("\"user_agent\":null"));
    }

    #[test]
    fn test_access_logger_default() {
        let logger = AccessLogger::default();
        // Just verify it does not panic when logging.
        let entry = sample_entry();
        logger.log(&entry);
    }
}
