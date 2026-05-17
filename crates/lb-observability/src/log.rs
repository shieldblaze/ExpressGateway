//! REL-2-06: central tracing/log subscriber initialisation.
//!
//! Selects between JSON and human-readable text formatters based on
//! the `LB_LOG_FORMAT` environment variable (default: `json`). All
//! binaries — `lb` today, future agents tomorrow — call
//! [`init_tracing`] once at the top of their `async_main` to get a
//! single canonical line format that the log shipper can rely on.
//!
//! Output schema (JSON mode, flattened):
//!
//! ```json
//! {
//!   "timestamp":"2026-05-13T09:31:12.512Z",
//!   "level":"INFO",
//!   "target":"lb::main",
//!   "message":"ExpressGateway v0.1.0",
//!   "version":"0.1.0"
//! }
//! ```
//!
//! Span context (`trace_id`, `span_id`) is folded in once REL-2-07
//! lands the OTLP layer; the JSON schema is forward-compatible
//! (extra keys, no removals).

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Wire-format selector for the subscriber.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// Newline-delimited JSON, one object per event. Default.
    Json,
    /// Human-readable text with ANSI colour codes off (so the format
    /// survives being piped to `journalctl` or a flat log file).
    Text,
}

impl LogFormat {
    /// Parse a case-insensitive string token. Unknown tokens return
    /// `None` so the caller can fall back to the configured default
    /// rather than silently picking text.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "json" => Some(Self::Json),
            "text" | "plain" => Some(Self::Text),
            _ => None,
        }
    }
}

/// Configuration knob set at startup.
#[derive(Clone, Debug)]
pub struct TracingConfig {
    /// Output format. Override via `LB_LOG_FORMAT` env var.
    pub format: LogFormat,
    /// Filter directive applied when `RUST_LOG` is unset.
    pub default_directive: String,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            format: LogFormat::Json,
            default_directive: "info".to_owned(),
        }
    }
}

/// Errors raised by [`init_tracing`].
#[derive(Debug, thiserror::Error)]
pub enum TracingError {
    /// A previous call to [`init_tracing`] (or any other call to
    /// `tracing::subscriber::set_global_default`) already installed a
    /// subscriber. `tracing-subscriber` only allows one per process.
    #[error("tracing subscriber already initialised")]
    AlreadyInitialised,
}

/// Marker so a second call to [`init_tracing`] from within the same
/// process returns [`TracingError::AlreadyInitialised`] cleanly rather
/// than calling `tracing-subscriber` again (which would panic).
static INIT: OnceLock<LogFormat> = OnceLock::new();

/// Install the global tracing subscriber.
///
/// Resolution order for the format:
///   1. `LB_LOG_FORMAT` env var (`json` | `text`).
///   2. `cfg.format`.
///
/// Resolution order for the filter:
///   1. `RUST_LOG` env var.
///   2. `cfg.default_directive`.
///
/// Idempotent — second and subsequent calls return
/// [`TracingError::AlreadyInitialised`].
///
/// # Errors
///
/// See [`TracingError`].
pub fn init_tracing(cfg: &TracingConfig) -> Result<(), TracingError> {
    let format = std::env::var("LB_LOG_FORMAT")
        .ok()
        .and_then(|s| LogFormat::parse(&s))
        .unwrap_or(cfg.format);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.default_directive));

    // `try_init()` is the fallible variant — returns an error rather
    // than panicking when a global subscriber is already installed.
    let install_result = match format {
        LogFormat::Json => fmt()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_span_list(false)
            .with_target(true)
            .with_env_filter(filter)
            .try_init(),
        LogFormat::Text => fmt()
            .with_target(true)
            .with_ansi(false)
            .with_env_filter(filter)
            .try_init(),
    };

    match install_result {
        Ok(()) => {
            // Cache the choice for `current_format()` introspection.
            let _ = INIT.set(format);
            Ok(())
        }
        Err(_) => Err(TracingError::AlreadyInitialised),
    }
}

/// What format was eventually installed? `None` before
/// [`init_tracing`] is called.
#[must_use]
pub fn current_format() -> Option<LogFormat> {
    INIT.get().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parser_accepts_canonical_tokens() {
        assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse(" text "), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("plain"), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("yaml"), None);
    }

    #[test]
    fn default_config_is_json_info() {
        let cfg = TracingConfig::default();
        assert_eq!(cfg.format, LogFormat::Json);
        assert_eq!(cfg.default_directive, "info");
    }
}
