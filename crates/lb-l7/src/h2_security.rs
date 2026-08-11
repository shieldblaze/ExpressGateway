//! HTTP/2 security thresholds surfaced to hyper's `http2::Builder`.
//!
//! Attack → knob: Rapid Reset (CVE-2023-44487) →
//! `max_pending_accept_reset_streams`; rapid reset after a local error
//! (RUSTSEC-2024-0003) → `max_local_error_reset_streams`; HPACK bomb →
//! `max_header_list_size`; SETTINGS/stream explosion →
//! `max_concurrent_streams`; zero-window stall → `keep_alive_timeout` +
//! `max_send_buf_size`. CONTINUATION flood (CVE-2024-27316) and PING flood are
//! enforced inside `h2` itself and are NOT configurable here.
//!
//! hyper is the wire ENFORCER; the `lb-h2` detector types stay the single
//! source of truth for the VALUES, so one `DEFAULT_*` edit propagates.

use std::time::Duration;

/// Thresholds for the live HTTP/2 listener, built from `lb-h2::security`.
#[derive(Debug, Clone, Copy)]
pub struct H2SecurityThresholds {
    /// Queued `RST_STREAM` pairs before hyper GOAWAYs `ENHANCE_YOUR_CALM`.
    pub max_pending_accept_reset_streams: usize,
    /// `RST_STREAM` frames from local (app-layer) errors before GOAWAY —
    /// a separate knob for RUSTSEC-2024-0003.
    pub max_local_error_reset_streams: usize,
    /// Concurrent-stream cap; bounds a SETTINGS-flood blast radius.
    pub max_concurrent_streams: u32,
    /// Decoded HPACK header-list cap (the `HpackBombDetector` absolute cap).
    pub max_header_list_size: u32,
    /// Per-stream send buffer; caps memory an attacker pins with a zero window.
    pub max_send_buf_size: usize,
    /// Server-initiated keep-alive PING interval; `None` disables.
    pub keep_alive_interval: Option<Duration>,
    /// PING-ACK deadline — the zero-window-stall reap. INERT unless
    /// `keep_alive_interval` is `Some`.
    pub keep_alive_timeout: Duration,
    /// Initial per-stream receive window (RFC 9113 default 65_535).
    pub initial_stream_window_size: u32,
    /// Initial connection-level receive window (hyper's documented default).
    pub initial_connection_window_size: u32,
}

impl Default for H2SecurityThresholds {
    fn default() -> Self {
        // Both reset knobs model the same DoS posture, hence one default.
        Self {
            max_pending_accept_reset_streams: lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize,
            max_local_error_reset_streams: lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize,
            max_concurrent_streams: 256,
            // 64 KiB HPACK cap (matches Pingora); per-header limits live in h2.
            max_header_list_size: 64 * 1024,
            max_send_buf_size: 64 * 1024,
            keep_alive_interval: Some(lb_h2::DEFAULT_ZERO_WINDOW_STALL_TIMEOUT),
            keep_alive_timeout: lb_h2::DEFAULT_ZERO_WINDOW_STALL_TIMEOUT,
            initial_stream_window_size: 65_535,
            initial_connection_window_size: 1 << 20,
        }
    }
}

impl H2SecurityThresholds {
    /// Build a threshold set with the project defaults.
    #[must_use]
    pub fn from_detector_defaults() -> Self {
        Self::default()
    }

    /// Apply this threshold set to hyper's `http2::Builder`.
    pub fn apply<E>(self, builder: &mut hyper::server::conn::http2::Builder<E>) {
        builder
            .max_pending_accept_reset_streams(self.max_pending_accept_reset_streams)
            .max_local_error_reset_streams(self.max_local_error_reset_streams)
            .max_concurrent_streams(self.max_concurrent_streams)
            .max_header_list_size(self.max_header_list_size)
            .max_send_buf_size(self.max_send_buf_size)
            .keep_alive_interval(self.keep_alive_interval)
            .keep_alive_timeout(self.keep_alive_timeout)
            .initial_stream_window_size(self.initial_stream_window_size)
            .initial_connection_window_size(self.initial_connection_window_size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_align_with_lb_h2_constants() {
        let t = H2SecurityThresholds::default();
        assert_eq!(
            t.max_pending_accept_reset_streams,
            lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize
        );
        assert_eq!(
            t.keep_alive_timeout,
            lb_h2::DEFAULT_ZERO_WINDOW_STALL_TIMEOUT
        );
        assert_eq!(t.initial_stream_window_size, 65_535);
    }

    #[test]
    fn apply_does_not_panic_with_defaults() {
        // The hyper setters take `Into<Option<_>>`, so a bad value still
        // type-checks; smoke-test that the chain accepts our defaults.
        use hyper_util::rt::{TokioExecutor, TokioTimer};
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        // `keep_alive_interval` requires a timer (as in h2_proxy.rs).
        builder.timer(TokioTimer::new());
        H2SecurityThresholds::default().apply(&mut builder);
    }
}
