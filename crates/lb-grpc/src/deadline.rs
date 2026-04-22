//! gRPC deadline (timeout) propagation.
//!
//! Parses and formats the `grpc-timeout` header value per the gRPC
//! specification: `Timeout = 1*DIGIT TimeUnit` where `TimeUnit` is one of
//! `H` (hours), `M` (minutes), `S` (seconds), `m` (milliseconds),
//! `u` (microseconds), `n` (nanoseconds).

use crate::GrpcError;

/// Utilities for gRPC deadline / timeout header handling.
pub struct GrpcDeadline;

impl GrpcDeadline {
    /// Parse a `grpc-timeout` header value into milliseconds.
    ///
    /// # Examples
    ///
    /// - `"5S"` -> 5000 ms
    /// - `"100m"` -> 100 ms
    /// - `"1H"` -> 3,600,000 ms
    /// - `"2M"` -> 120,000 ms
    /// - `"1000000u"` -> 1000 ms
    /// - `"1000000000n"` -> 1000 ms
    ///
    /// # Errors
    ///
    /// Returns [`GrpcError::InvalidTimeout`] if the value does not match the
    /// expected format.
    pub fn parse_timeout(value: &str) -> Result<u64, GrpcError> {
        if value.is_empty() {
            return Err(GrpcError::InvalidTimeout(value.to_owned()));
        }

        // The last character is the unit; everything before it is the digit string.
        let (digits_str, unit_char) = value.split_at(value.len() - 1);

        if digits_str.is_empty() {
            return Err(GrpcError::InvalidTimeout(value.to_owned()));
        }

        let digits: u64 = digits_str
            .parse()
            .map_err(|_| GrpcError::InvalidTimeout(value.to_owned()))?;

        // Convert to milliseconds based on unit.
        // For sub-millisecond units (microseconds, nanoseconds), use ceiling
        // division so that a non-zero value never truncates to 0ms.
        let ms = match unit_char {
            "H" => digits.saturating_mul(3_600_000),
            "M" => digits.saturating_mul(60_000),
            "S" => digits.saturating_mul(1_000),
            "m" => digits,
            "u" => digits.saturating_add(999) / 1_000,
            "n" => digits.saturating_add(999_999) / 1_000_000,
            _ => return Err(GrpcError::InvalidTimeout(value.to_owned())),
        };

        Ok(ms)
    }

    /// Format a millisecond timeout into a `grpc-timeout` header value.
    ///
    /// Uses the most human-readable unit that represents the value exactly when
    /// possible. Falls back to milliseconds.
    #[must_use]
    pub fn format_timeout(timeout_ms: u64) -> String {
        if timeout_ms == 0 {
            return "0m".to_owned();
        }

        if timeout_ms % 3_600_000 == 0 {
            return format!("{}H", timeout_ms / 3_600_000);
        }
        if timeout_ms % 60_000 == 0 {
            return format!("{}M", timeout_ms / 60_000);
        }
        if timeout_ms % 1_000 == 0 {
            return format!("{}S", timeout_ms / 1_000);
        }

        format!("{timeout_ms}m")
    }

    /// Calculate remaining timeout given the original timeout and elapsed time.
    ///
    /// Returns `None` if the deadline has already passed (elapsed >= original).
    #[must_use]
    pub const fn remaining(original_ms: u64, elapsed_ms: u64) -> Option<u64> {
        if elapsed_ms >= original_ms {
            return None;
        }
        Some(original_ms - elapsed_ms)
    }
}
