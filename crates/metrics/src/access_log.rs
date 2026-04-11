//! Structured access logging via `tracing`.
//!
//! Each proxied request produces an [`AccessLogEntry`] that is emitted as a
//! structured `tracing` event at `INFO` level. Fields are emitted directly
//! into the span rather than serialising to an intermediate JSON `String`,
//! eliminating a heap allocation per request on the hot path.
//!
//! Sampling is supported: the [`AccessLogger`] can be configured to log only
//! every Nth request, reducing I/O pressure under high load while still
//! providing representative traffic visibility.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// A single access-log record.
///
/// All fields use `String` because the entry may be serialised to JSON for
/// export. For the hot-path logging codepath, the logger reads fields by
/// reference and writes them as structured tracing fields (zero intermediate
/// allocation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessLogEntry {
    /// ISO-8601 timestamp.
    pub timestamp: String,
    /// Listener name that accepted this request.
    pub listener: String,
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
    /// Unique request identifier (correlation ID).
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
    ///
    /// This is provided for export / testing. The primary logging path uses
    /// structured `tracing` fields and does not call this method.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

/// Structured access logger that emits entries via [`tracing`].
///
/// Supports deterministic 1-in-N sampling to reduce log volume.
pub struct AccessLogger {
    /// Log every Nth request.  1 = log all, 10 = log 10%, etc.
    sample_rate: u64,
    /// Monotonic counter used for sampling.
    counter: AtomicU64,
}

impl AccessLogger {
    /// Create a new `AccessLogger` that logs every request (no sampling).
    pub fn new() -> Self {
        Self {
            sample_rate: 1,
            counter: AtomicU64::new(0),
        }
    }

    /// Create an `AccessLogger` that logs 1 out of every `n` requests.
    ///
    /// A `sample_rate` of 0 or 1 logs every request.
    pub fn with_sample_rate(n: u64) -> Self {
        Self {
            sample_rate: n.max(1),
            counter: AtomicU64::new(0),
        }
    }

    /// Log an access-log entry at `INFO` level using structured tracing
    /// fields.
    ///
    /// If sampling is enabled, only every Nth call actually emits a log
    /// event.  The decision is deterministic (based on a monotonic counter)
    /// so it is reproducible and does not require a PRNG.
    #[inline]
    pub fn log(&self, entry: &AccessLogEntry) {
        // Fast path: bump counter and check sample.
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        if !seq.is_multiple_of(self.sample_rate) {
            return;
        }

        // Emit structured fields directly -- no JSON serialisation.
        tracing::info!(
            target: "access_log",
            timestamp = %entry.timestamp,
            listener = %entry.listener,
            method = %entry.method,
            uri = %entry.uri,
            status = entry.status,
            latency_ms = entry.latency_ms,
            bytes_sent = entry.bytes_sent,
            bytes_received = entry.bytes_received,
            request_id = %entry.request_id,
            protocol = %entry.protocol,
            tls_version = entry.tls_version.as_deref().unwrap_or("-"),
            tls_cipher = entry.tls_cipher.as_deref().unwrap_or("-"),
            client_ip = %entry.client_ip,
            backend = %entry.backend,
            user_agent = entry.user_agent.as_deref().unwrap_or("-"),
            "access"
        );
    }

    /// The configured sample rate.
    #[inline]
    pub fn sample_rate(&self) -> u64 {
        self.sample_rate
    }

    /// Total number of requests seen (including those not logged).
    #[inline]
    pub fn total_seen(&self) -> u64 {
        self.counter.load(Ordering::Relaxed)
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
            listener: "default".to_string(),
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
        assert_eq!(parsed.listener, "default");
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
        assert!(json.contains("\"listener\":\"default\""));
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
        let entry = sample_entry();
        logger.log(&entry);
        assert_eq!(logger.total_seen(), 1);
    }

    #[test]
    fn test_sampling_logs_every_nth() {
        let logger = AccessLogger::with_sample_rate(10);
        let entry = sample_entry();

        for _ in 0..100 {
            logger.log(&entry);
        }

        assert_eq!(logger.total_seen(), 100);
        assert_eq!(logger.sample_rate(), 10);
    }

    #[test]
    fn test_sampling_rate_zero_treated_as_one() {
        let logger = AccessLogger::with_sample_rate(0);
        assert_eq!(logger.sample_rate(), 1);
    }

    #[test]
    fn test_sampling_rate_one_logs_all() {
        let logger = AccessLogger::with_sample_rate(1);
        let entry = sample_entry();

        for _ in 0..10 {
            logger.log(&entry);
        }
        assert_eq!(logger.total_seen(), 10);
    }
}
