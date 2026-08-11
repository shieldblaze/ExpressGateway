//! gRPC deadline propagation: `Timeout = 1*DIGIT TimeUnit`, where `TimeUnit`
//! is `H`/`M`/`S`/`m`/`u`/`n`.

use crate::GrpcError;

/// Utilities for gRPC deadline / timeout header handling.
pub struct GrpcDeadline;

impl GrpcDeadline {
    /// Parse a `grpc-timeout` value into milliseconds.
    ///
    /// # Errors
    /// [`GrpcError::InvalidTimeout`] if it does not match the grammar.
    pub fn parse_timeout(value: &str) -> Result<u64, GrpcError> {
        if value.is_empty() {
            return Err(GrpcError::InvalidTimeout(value.to_owned()));
        }

        let (digits_str, unit_char) = value.split_at(value.len() - 1);

        if digits_str.is_empty() {
            return Err(GrpcError::InvalidTimeout(value.to_owned()));
        }

        let digits: u64 = digits_str
            .parse()
            .map_err(|_| GrpcError::InvalidTimeout(value.to_owned()))?;

        // CEILING division: a non-zero sub-ms value must never become 0 ms.
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

    /// Format milliseconds as a `grpc-timeout` value, preferring the coarsest
    /// unit that divides evenly.
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

    /// Remaining timeout, or `None` once the deadline has passed.
    #[must_use]
    pub const fn remaining(original_ms: u64, elapsed_ms: u64) -> Option<u64> {
        if elapsed_ms >= original_ms {
            return None;
        }
        Some(original_ms - elapsed_ms)
    }
}
