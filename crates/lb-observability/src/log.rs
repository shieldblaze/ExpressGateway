//! Central tracing/log subscriber init; JSON or text per `LB_LOG_FORMAT` (default `json`).
//!
//! The JSON schema is forward-compatible by contract: keys may be ADDED, never removed or
//! renamed, because log shippers parse it.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Wire-format selector for the subscriber.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// Newline-delimited JSON, one object per event. Default.
    Json,
    /// Human-readable text, ANSI off so it survives `journalctl` and flat files.
    Text,
}

impl LogFormat {
    /// Parse a case-insensitive token; `None` on unknown so the caller applies its own default.
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
    /// A subscriber is already installed; `tracing-subscriber` allows one per process.
    #[error("tracing subscriber already initialised")]
    AlreadyInitialised,
}

/// Guards against a second `tracing-subscriber` install, which would panic.
static INIT: OnceLock<LogFormat> = OnceLock::new();

/// Install the global subscriber. The ENV WINS over `cfg` for both format (`LB_LOG_FORMAT`) and
/// filter (`RUST_LOG`). Idempotent.
pub fn init_tracing(cfg: &TracingConfig) -> Result<(), TracingError> {
    let format = std::env::var("LB_LOG_FORMAT")
        .ok()
        .and_then(|s| LogFormat::parse(&s))
        .unwrap_or(cfg.format);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.default_directive));

    // The fallible variant — `init()` would panic on a second install.
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

/// The installed format; `None` before [`init_tracing`].
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
