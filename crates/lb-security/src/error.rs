//! Security error types.

/// Errors raised by security mitigation checks.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// CL-TE request smuggling: both Content-Length and Transfer-Encoding present.
    #[error("request smuggling detected: CL-TE conflict")]
    SmuggleCLTE,

    /// TE-CL request smuggling: Transfer-Encoding final encoding is not chunked.
    #[error("request smuggling detected: TE-CL ambiguity")]
    SmuggleTECL,

    /// Duplicate Content-Length headers with differing values.
    #[error("request smuggling detected: duplicate Content-Length with differing values")]
    SmuggleDuplicateCL,

    /// H2 downgrade smuggling: prohibited headers present in H2-to-H1 translation.
    #[error("request smuggling detected: H2 downgrade with prohibited headers")]
    SmuggleH2Downgrade,

    /// Slowloris: header phase exceeded the configured timeout.
    #[error("slowloris detected: header timeout exceeded ({elapsed_ms}ms > {timeout_ms}ms)")]
    SlowlorisTimeout {
        /// Elapsed time in milliseconds.
        elapsed_ms: u64,
        /// Configured timeout in milliseconds.
        timeout_ms: u64,
    },

    /// Slowloris: header receive rate below minimum threshold.
    #[error("slowloris detected: header rate too low ({rate_bps} B/s < {min_rate_bps} B/s)")]
    SlowlorisRate {
        /// Observed rate in bytes per second.
        rate_bps: u64,
        /// Configured minimum rate in bytes per second.
        min_rate_bps: u64,
    },

    /// Slow POST: body receive rate below minimum threshold.
    #[error("slow POST detected: body rate too low ({rate_bps} B/s < {min_rate_bps} B/s)")]
    SlowPost {
        /// Observed rate in bytes per second.
        rate_bps: u64,
        /// Configured minimum rate in bytes per second.
        min_rate_bps: u64,
    },

    /// 0-RTT replay: token has been seen before.
    #[error("0-RTT replay detected")]
    ZeroRttReplay,
}
