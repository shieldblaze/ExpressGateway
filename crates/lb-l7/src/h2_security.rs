//! HTTP/2 security thresholds surfaced to hyper's `http2::Builder`.
//!
//! The six detector types in `lb-h2::security` carry the canonical
//! thresholds for the HTTP/2 flood / bomb attacks the gateway must
//! mitigate:
//!
//! | Attack / CVE                                  | Detector type                  | Hyper knob                              |
//! |-----------------------------------------------|--------------------------------|-----------------------------------------|
//! | Rapid Reset (CVE-2023-44487)                  | [`RapidResetDetector`]         | `max_pending_accept_reset_streams`      |
//! | Rapid Reset after local error (RUSTSEC-2024-0003)| —                           | `max_local_error_reset_streams`         |
//! | CONTINUATION Flood (CVE-2024-27316)           | [`ContinuationFloodDetector`]  | enforced inside `h2 ≥ 0.4.5`            |
//! | HPACK Bomb                                    | [`HpackBombDetector`]          | `max_header_list_size`                  |
//! | SETTINGS flood (stream explosion)             | [`SettingsFloodDetector`]      | `max_concurrent_streams`                |
//! | PING flood                                    | [`PingFloodDetector`]          | enforced by `h2` (unconfigurable, safe) |
//! | Zero-window stall                             | [`ZeroWindowStallDetector`]    | `keep_alive_timeout` + `max_send_buf_size` |
//!
//! Hyper is the **wire enforcer**; the lb-h2 detector types remain the
//! single source of truth for threshold values so a change to one
//! `DEFAULT_*` constant propagates to the live listener without config
//! drift.
//!
//! [`RapidResetDetector`]: ../../lb_h2/security/struct.RapidResetDetector.html
//! [`ContinuationFloodDetector`]: ../../lb_h2/security/struct.ContinuationFloodDetector.html
//! [`HpackBombDetector`]: ../../lb_h2/security/struct.HpackBombDetector.html
//! [`SettingsFloodDetector`]: ../../lb_h2/security/struct.SettingsFloodDetector.html
//! [`PingFloodDetector`]: ../../lb_h2/security/struct.PingFloodDetector.html
//! [`ZeroWindowStallDetector`]: ../../lb_h2/security/struct.ZeroWindowStallDetector.html

use std::time::Duration;

/// Batched thresholds for the live HTTP/2 listener.
///
/// Built via [`Self::default`] from the `lb-h2::security` constants.
/// Threaded into [`crate::h2_proxy::H2Proxy::new`] and applied to
/// hyper's `http2::Builder` inside `serve_connection`.
#[derive(Debug, Clone, Copy)]
pub struct H2SecurityThresholds {
    /// Maximum number of server-initiated / client-initiated `RST_STREAM`
    /// pairs hyper will queue before sending GOAWAY `ENHANCE_YOUR_CALM`.
    /// Mirrors `RapidResetDetector` threshold.
    pub max_pending_accept_reset_streams: usize,
    /// Maximum `RST_STREAM` frames emitted due to local (app-layer) errors
    /// before GOAWAY. Separate knob added for `RUSTSEC-2024-0003`.
    pub max_local_error_reset_streams: usize,
    /// Maximum concurrent streams the server will accept. Caps the
    /// blast radius of a SETTINGS flood that inflates stream counts.
    pub max_concurrent_streams: u32,
    /// Maximum size (bytes) of a decoded HPACK header list.
    /// Equivalent to the `HpackBombDetector` absolute cap.
    pub max_header_list_size: u32,
    /// Maximum per-stream send buffer. Caps the memory an attacker can
    /// pin by advertising a zero window and refusing to read.
    pub max_send_buf_size: usize,
    /// Interval between server-initiated H2 keep-alive PINGs. `None`
    /// disables the keep-alive mechanism. When set together with
    /// `keep_alive_timeout`, the connection is closed if the peer
    /// fails to ACK within the timeout.
    pub keep_alive_interval: Option<Duration>,
    /// Close a connection whose peer has not `ACK`ed a PING within this
    /// period. Fires the zero-window stall on an attacker that holds a
    /// stream open without granting credit. Only takes effect when
    /// `keep_alive_interval` is `Some`.
    pub keep_alive_timeout: Duration,
    /// Initial per-stream receive window. Default matches RFC 9113
    /// (`SETTINGS_INITIAL_WINDOW_SIZE` = `65_535`).
    pub initial_stream_window_size: u32,
    /// Initial connection-level receive window. 1 MiB is hyper's
    /// documented default and a reasonable safe starting point.
    pub initial_connection_window_size: u32,
}

impl Default for H2SecurityThresholds {
    fn default() -> Self {
        // The lb-h2 constants are `u32`/`Duration`; widen to `usize`
        // where hyper wants `usize`. The rapid-reset threshold is drawn
        // from the same flood defaults — 100 per 10s window. We
        // deliberately reuse that number for both reset-stream knobs
        // because they model the same DoS posture.
        Self {
            max_pending_accept_reset_streams: lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize,
            max_local_error_reset_streams: lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize,
            max_concurrent_streams: 256,
            // 64 KiB HPACK cap — matches Pingora and conservative
            // production deployments. Absolute cap; per-header limits
            // are enforced inside h2's decoder.
            max_header_list_size: 64 * 1024,
            // 64 KiB per-stream send buffer.
            max_send_buf_size: 64 * 1024,
            // Ping every 30 s; close if no ACK in 30 s — matches the
            // `ZeroWindowStallDetector` default.
            keep_alive_interval: Some(lb_h2::DEFAULT_ZERO_WINDOW_STALL_TIMEOUT),
            keep_alive_timeout: lb_h2::DEFAULT_ZERO_WINDOW_STALL_TIMEOUT,
            initial_stream_window_size: 65_535,
            initial_connection_window_size: 1 << 20,
        }
    }
}

impl H2SecurityThresholds {
    /// Build a threshold set with the project-default values. Thin
    /// wrapper over [`Default::default`] that reads as an explicit
    /// "pull from the `lb-h2` security defaults" at call sites.
    #[must_use]
    pub fn from_detector_defaults() -> Self {
        Self::default()
    }

    /// Apply this threshold set to hyper's `http2::Builder`. The
    /// generic over `E` lets us stay agnostic about which executor the
    /// caller wired (today always `hyper_util::rt::TokioExecutor`).
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
        // Regression: the hyper Builder setters consume `Into<Option<_>>`
        // so a `0` or `u32::MAX` could look valid but still produce
        // weird wire behavior. Cheap smoke test that the chain accepts
        // our defaults.
        use hyper_util::rt::{TokioExecutor, TokioTimer};
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        // `keep_alive_interval` requires a timer; wire the tokio one
        // for parity with h2_proxy.rs.
        builder.timer(TokioTimer::new());
        H2SecurityThresholds::default().apply(&mut builder);
    }
}
