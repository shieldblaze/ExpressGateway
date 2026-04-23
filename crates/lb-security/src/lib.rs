//! Security mitigations: denial-of-service protection, request smuggling prevention, rate limiting.
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
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
mod slow_post;
mod slowloris;
mod smuggle;
mod ticket;
mod zero_rtt;

pub use error::SecurityError;
pub use slow_post::SlowPostDetector;
pub use slowloris::SlowlorisDetector;
pub use smuggle::SmuggleDetector;
pub use ticket::{RotatingTicketer, TicketError, TicketKey, TicketRotator, build_server_config};
pub use zero_rtt::ZeroRttReplayGuard;

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Smuggle: CL-TE ----

    #[test]
    fn smuggle_cl_te_both_present_rejected() {
        let headers = vec![
            ("content-length".into(), "10".into()),
            ("transfer-encoding".into(), "chunked".into()),
        ];
        assert!(SmuggleDetector::check_cl_te(&headers).is_err());
    }

    #[test]
    fn smuggle_cl_only_ok() {
        let headers = vec![("content-length".into(), "10".into())];
        assert!(SmuggleDetector::check_cl_te(&headers).is_ok());
    }

    // ---- Smuggle: TE-CL ----

    #[test]
    fn smuggle_te_cl_non_chunked_final_rejected() {
        let headers = vec![("transfer-encoding".into(), "gzip, identity".into())];
        assert!(SmuggleDetector::check_te_cl(&headers).is_err());
    }

    #[test]
    fn smuggle_te_cl_chunked_final_ok() {
        let headers = vec![("transfer-encoding".into(), "gzip, chunked".into())];
        assert!(SmuggleDetector::check_te_cl(&headers).is_ok());
    }

    // ---- Smuggle: duplicate CL ----

    #[test]
    fn smuggle_duplicate_cl_differing_values_rejected() {
        let headers = vec![
            ("content-length".into(), "10".into()),
            ("content-length".into(), "20".into()),
        ];
        assert!(SmuggleDetector::check_duplicate_cl(&headers).is_err());
    }

    #[test]
    fn smuggle_duplicate_cl_same_values_ok() {
        let headers = vec![
            ("content-length".into(), "10".into()),
            ("content-length".into(), "10".into()),
        ];
        assert!(SmuggleDetector::check_duplicate_cl(&headers).is_ok());
    }

    #[test]
    fn smuggle_single_cl_ok() {
        let headers = vec![("content-length".into(), "42".into())];
        assert!(SmuggleDetector::check_duplicate_cl(&headers).is_ok());
    }

    // ---- Smuggle: H2 downgrade ----

    #[test]
    fn smuggle_h2_downgrade_connection_header_rejected() {
        let headers = vec![("connection".into(), "keep-alive".into())];
        assert!(SmuggleDetector::check_h2_downgrade(&headers, true).is_err());
    }

    #[test]
    fn smuggle_h2_downgrade_not_h2_ok() {
        let headers = vec![("connection".into(), "keep-alive".into())];
        assert!(SmuggleDetector::check_h2_downgrade(&headers, false).is_ok());
    }

    #[test]
    fn smuggle_h2_downgrade_te_non_trailers_rejected() {
        let headers = vec![("te".into(), "gzip".into())];
        assert!(SmuggleDetector::check_h2_downgrade(&headers, true).is_err());
    }

    #[test]
    fn smuggle_h2_downgrade_te_trailers_ok() {
        let headers = vec![("te".into(), "trailers".into())];
        assert!(SmuggleDetector::check_h2_downgrade(&headers, true).is_ok());
    }

    #[test]
    fn smuggle_h2_downgrade_te_trailers_case_insensitive_ok() {
        let headers = vec![("te".into(), "Trailers".into())];
        assert!(SmuggleDetector::check_h2_downgrade(&headers, true).is_ok());
    }

    #[test]
    fn smuggle_check_all_clean_headers_ok() {
        let headers = vec![
            ("host".into(), "example.com".into()),
            ("accept".into(), "*/*".into()),
        ];
        assert!(SmuggleDetector::check_all(&headers, false).is_ok());
        assert!(SmuggleDetector::check_all(&headers, true).is_ok());
    }

    // ---- Slowloris ----

    #[test]
    fn slowloris_timeout_exceeded() {
        let detector = SlowlorisDetector::new(5000, 100);
        assert!(detector.check_header_timeout(6000).is_err());
    }

    #[test]
    fn slowloris_within_timeout() {
        let detector = SlowlorisDetector::new(5000, 100);
        assert!(detector.check_header_timeout(1000).is_ok());
    }

    #[test]
    fn slowloris_rate_below_min_windowed() {
        let mut detector = SlowlorisDetector::new(10000, 100);
        // First window: 500 bytes at t=1000ms => 500 B/s, OK
        assert!(detector.record_bytes(500, 1000).is_ok());
        // Second window: only 10 more bytes at t=3000ms => 10 bytes / 2000ms = 5 B/s
        assert!(detector.record_bytes(510, 3000).is_err());
    }

    #[test]
    fn slowloris_rate_above_min_windowed() {
        let mut detector = SlowlorisDetector::new(10000, 100);
        // First window: 1000 bytes at t=1000ms => 1000 B/s, above 100 B/s
        assert!(detector.record_bytes(1000, 1000).is_ok());
        // Second window: 2000 more bytes at t=2000ms => 1000 bytes / 1000ms = 1000 B/s
        assert!(detector.record_bytes(2000, 2000).is_ok());
    }

    #[test]
    fn slowloris_window_too_short_skipped() {
        let mut detector = SlowlorisDetector::new(10000, 100);
        // Window < 1000ms: should skip the check and return Ok.
        assert!(detector.record_bytes(1, 500).is_ok());
    }

    // ---- Slow POST ----

    #[test]
    fn slow_post_below_threshold_windowed() {
        let mut detector = SlowPostDetector::new(10000, 100);
        // First window: 500 bytes at t=1000ms => 500 B/s, OK
        assert!(detector.record_body_bytes(500, 1000, 10000).is_ok());
        // Second window: only 5 more bytes at t=3000ms => 5 bytes / 2000ms = 2 B/s
        assert!(detector.record_body_bytes(505, 3000, 10000).is_err());
    }

    #[test]
    fn slow_post_above_threshold_windowed() {
        let mut detector = SlowPostDetector::new(10000, 100);
        // First window: 5000 bytes at t=1000ms => 5000 B/s, OK
        assert!(detector.record_body_bytes(5000, 1000, 10000).is_ok());
        // Second window: 5000 more bytes at t=2000ms => 5000 bytes / 1000ms = 5000 B/s
        assert!(detector.record_body_bytes(10000, 2000, 10000).is_ok());
    }

    #[test]
    fn slow_post_timeout_exceeded() {
        let mut detector = SlowPostDetector::new(5000, 100);
        // Even with decent rate, exceeding timeout triggers error.
        assert!(detector.record_body_bytes(100_000, 6000, 200_000).is_err());
    }

    // ---- 0-RTT replay ----

    #[test]
    fn zero_rtt_replay_detected() {
        let mut guard = ZeroRttReplayGuard::new(1000);
        let token = b"unique-token";
        assert!(guard.check_and_record(token).is_ok());
        assert!(guard.check_and_record(token).is_err());
    }

    #[test]
    fn zero_rtt_different_tokens_ok() {
        let mut guard = ZeroRttReplayGuard::new(1000);
        assert!(guard.check_and_record(b"token-a").is_ok());
        assert!(guard.check_and_record(b"token-b").is_ok());
    }

    #[test]
    fn zero_rtt_eviction_on_capacity() {
        let mut guard = ZeroRttReplayGuard::new(2);
        assert!(guard.check_and_record(b"a").is_ok());
        assert!(guard.check_and_record(b"b").is_ok());
        // "a" should be evicted when "c" comes in
        assert!(guard.check_and_record(b"c").is_ok());
        // "a" was evicted, so it should be accepted again
        assert!(guard.check_and_record(b"a").is_ok());
    }

    #[test]
    fn zero_rtt_hash_based_dedup() {
        // Verify that the hash-based approach still correctly deduplicates.
        let mut guard = ZeroRttReplayGuard::new(100);
        let token = b"some-long-token-value-that-would-be-expensive-to-clone";
        assert!(guard.check_and_record(token).is_ok());
        assert!(guard.check_and_record(token).is_err());
        // Different token with same prefix should be fine.
        assert!(
            guard
                .check_and_record(b"some-long-token-value-that-would-be-expensive-to-clone!")
                .is_ok()
        );
    }
}
