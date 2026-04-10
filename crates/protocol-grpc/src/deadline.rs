//! gRPC deadline (timeout) handling.
//!
//! Parses the `grpc-timeout` header whose format is `<value><unit>` where
//! `<value>` is one or more ASCII digits and `<unit>` is one of:
//!
//! | Unit | Meaning      |
//! |------|-------------|
//! | `H`  | Hours       |
//! | `M`  | Minutes     |
//! | `S`  | Seconds     |
//! | `m`  | Milliseconds|
//! | `u`  | Microseconds|
//! | `n`  | Nanoseconds |
//!
//! The maximum deadline is clamped to [`MAX_DEADLINE`] (300 seconds).
//!
//! Per the gRPC spec, `grpc-timeout` values have at most 8 digits.

use std::time::Duration;

use http::HeaderMap;

/// Maximum allowed deadline (5 minutes).
pub const MAX_DEADLINE: Duration = Duration::from_secs(300);

/// Maximum number of digits allowed in a grpc-timeout value per spec.
const MAX_TIMEOUT_DIGITS: usize = 8;

/// Errors that can occur while parsing a `grpc-timeout` value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeadlineError {
    #[error("missing grpc-timeout header")]
    Missing,

    #[error("invalid grpc-timeout value: {0}")]
    Invalid(String),

    #[error("unknown time unit: {0}")]
    UnknownUnit(char),
}

/// Parsed (and clamped) gRPC deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    duration: Duration,
}

impl Deadline {
    /// Returns the clamped duration.
    #[inline]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Parse from a raw `grpc-timeout` header value string.
    ///
    /// Validates format: 1-8 ASCII digits followed by exactly one unit char.
    pub fn parse(value: &str) -> Result<Self, DeadlineError> {
        if value.is_empty() {
            return Err(DeadlineError::Invalid(value.to_string()));
        }

        // Split into digits and unit character.
        let unit_pos = value
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| DeadlineError::Invalid(value.to_string()))?;

        let digits = &value[..unit_pos];
        let unit_str = &value[unit_pos..];

        if digits.is_empty() || unit_str.len() != 1 || digits.len() > MAX_TIMEOUT_DIGITS {
            return Err(DeadlineError::Invalid(value.to_string()));
        }

        let amount: u64 = digits
            .parse()
            .map_err(|_| DeadlineError::Invalid(value.to_string()))?;

        // unit_str is exactly 1 byte of ASCII, safe to index.
        let unit = unit_str.as_bytes()[0];
        let duration = match unit {
            b'H' => Duration::from_secs(amount.saturating_mul(3600)),
            b'M' => Duration::from_secs(amount.saturating_mul(60)),
            b'S' => Duration::from_secs(amount),
            b'm' => Duration::from_millis(amount),
            b'u' => Duration::from_micros(amount),
            b'n' => Duration::from_nanos(amount),
            other => return Err(DeadlineError::UnknownUnit(other as char)),
        };

        Ok(Self {
            duration: duration.min(MAX_DEADLINE),
        })
    }

    /// Extract the deadline from the `grpc-timeout` header in the given
    /// header map.
    pub fn from_headers(headers: &HeaderMap) -> Result<Self, DeadlineError> {
        let value = headers
            .get("grpc-timeout")
            .ok_or(DeadlineError::Missing)?
            .to_str()
            .map_err(|e| DeadlineError::Invalid(e.to_string()))?;
        Self::parse(value)
    }

    /// Encode this deadline as a `grpc-timeout` header value string.
    ///
    /// Uses the largest unit that represents the duration without loss,
    /// preferring seconds for most values.
    pub fn to_header_value(&self) -> String {
        let nanos = self.duration.as_nanos();
        if nanos == 0 {
            return "0n".to_string();
        }

        let secs = self.duration.as_secs();
        let subsec_nanos = self.duration.subsec_nanos();

        if subsec_nanos == 0 {
            // Whole seconds -- pick the largest clean unit.
            if secs.is_multiple_of(3600) && secs / 3600 <= 99_999_999 {
                return format!("{}H", secs / 3600);
            }
            if secs.is_multiple_of(60) && secs / 60 <= 99_999_999 {
                return format!("{}M", secs / 60);
            }
            if secs <= 99_999_999 {
                return format!("{secs}S");
            }
        }

        let millis = self.duration.as_millis();
        if millis * 1_000_000 == nanos && millis <= 99_999_999 {
            return format!("{millis}m");
        }

        let micros = self.duration.as_micros();
        if micros * 1_000 == nanos && micros <= 99_999_999 {
            return format!("{micros}u");
        }

        if nanos <= 99_999_999 {
            return format!("{nanos}n");
        }

        // Fallback: use milliseconds, truncating sub-ms precision.
        format!("{}m", millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hours() {
        let d = Deadline::parse("1H").unwrap();
        // Clamped to 300s
        assert_eq!(d.duration(), MAX_DEADLINE);
    }

    #[test]
    fn parse_minutes() {
        let d = Deadline::parse("2M").unwrap();
        assert_eq!(d.duration(), Duration::from_secs(120));
    }

    #[test]
    fn parse_seconds() {
        let d = Deadline::parse("30S").unwrap();
        assert_eq!(d.duration(), Duration::from_secs(30));
    }

    #[test]
    fn parse_milliseconds() {
        let d = Deadline::parse("500m").unwrap();
        assert_eq!(d.duration(), Duration::from_millis(500));
    }

    #[test]
    fn parse_microseconds() {
        let d = Deadline::parse("1000u").unwrap();
        assert_eq!(d.duration(), Duration::from_micros(1000));
    }

    #[test]
    fn parse_nanoseconds() {
        let d = Deadline::parse("999999n").unwrap();
        assert_eq!(d.duration(), Duration::from_nanos(999999));
    }

    #[test]
    fn clamp_at_300_seconds() {
        // 10 minutes = 600s -> should clamp to 300s
        let d = Deadline::parse("10M").unwrap();
        assert_eq!(d.duration(), MAX_DEADLINE);

        // 301 seconds -> clamped
        let d = Deadline::parse("301S").unwrap();
        assert_eq!(d.duration(), MAX_DEADLINE);

        // 500_000 milliseconds = 500s -> clamped
        let d = Deadline::parse("500000m").unwrap();
        assert_eq!(d.duration(), MAX_DEADLINE);
    }

    #[test]
    fn exactly_300_not_clamped() {
        let d = Deadline::parse("300S").unwrap();
        assert_eq!(d.duration(), Duration::from_secs(300));
    }

    #[test]
    fn unknown_unit_error() {
        assert_eq!(Deadline::parse("10x"), Err(DeadlineError::UnknownUnit('x')));
    }

    #[test]
    fn empty_value_error() {
        assert!(Deadline::parse("").is_err());
    }

    #[test]
    fn no_unit_error() {
        assert!(Deadline::parse("100").is_err());
    }

    #[test]
    fn too_many_digits_error() {
        // 9 digits exceeds the 8-digit limit
        assert!(Deadline::parse("123456789S").is_err());
    }

    #[test]
    fn max_digits_ok() {
        // 8 digits is fine
        let d = Deadline::parse("12345678n").unwrap();
        assert_eq!(d.duration(), Duration::from_nanos(12345678));
    }

    #[test]
    fn from_headers_missing() {
        let headers = HeaderMap::new();
        assert_eq!(
            Deadline::from_headers(&headers),
            Err(DeadlineError::Missing)
        );
    }

    #[test]
    fn from_headers_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-timeout", "5S".parse().unwrap());
        let d = Deadline::from_headers(&headers).unwrap();
        assert_eq!(d.duration(), Duration::from_secs(5));
    }

    #[test]
    fn to_header_value_seconds() {
        let d = Deadline::parse("30S").unwrap();
        assert_eq!(d.to_header_value(), "30S");
    }

    #[test]
    fn to_header_value_millis() {
        let d = Deadline::parse("500m").unwrap();
        assert_eq!(d.to_header_value(), "500m");
    }

    #[test]
    fn to_header_value_roundtrip() {
        for input in &["5S", "100m", "1000u", "999n", "2M"] {
            let d = Deadline::parse(input).unwrap();
            let encoded = d.to_header_value();
            let d2 = Deadline::parse(&encoded).unwrap();
            assert_eq!(d.duration(), d2.duration(), "roundtrip failed for {input}");
        }
    }
}
