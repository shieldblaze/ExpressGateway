//! HTTP request smuggling detection.
//!
//! Implements checks per RFC 9112 section 6.1 and the H2-to-H1 downgrade
//! rules from RFC 9113 section 8.2.2.

use crate::SecurityError;

/// Stateless detector for HTTP request smuggling attack patterns.
pub struct SmuggleDetector;

impl SmuggleDetector {
    /// Check for duplicate Content-Length headers with differing values.
    ///
    /// RFC 9110 section 8.6: If a message is received with multiple
    /// Content-Length header fields having differing values, the recipient
    /// MUST reject the message as invalid.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SmuggleDuplicateCL`] if multiple Content-Length
    /// headers are present and their values differ.
    pub fn check_duplicate_cl(headers: &[(String, String)]) -> Result<(), SecurityError> {
        let mut first_value: Option<&str> = None;
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-length") {
                match first_value {
                    None => first_value = Some(value.trim()),
                    Some(prev) => {
                        if prev != value.trim() {
                            return Err(SecurityError::SmuggleDuplicateCL);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Check for CL-TE smuggling: Content-Length present AND Transfer-Encoding present.
    ///
    /// RFC 9112 section 6.1: A server MUST NOT apply both in a way that could lead to
    /// ambiguity. We reject the request outright if both are present.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SmuggleCLTE`] if both Content-Length and
    /// Transfer-Encoding headers are present.
    pub fn check_cl_te(headers: &[(String, String)]) -> Result<(), SecurityError> {
        let has_cl = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"));
        let has_te = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding"));

        if has_cl && has_te {
            return Err(SecurityError::SmuggleCLTE);
        }
        Ok(())
    }

    /// Check for TE-CL smuggling: Transfer-Encoding present but the final
    /// encoding is not `chunked`.
    ///
    /// RFC 9112 section 6.1: If Transfer-Encoding is present, the final
    /// encoding MUST be chunked. A non-chunked final encoding signals a
    /// potential smuggling vector.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SmuggleTECL`] if Transfer-Encoding is present
    /// and the last encoding in the comma-separated list is not `chunked`.
    pub fn check_te_cl(headers: &[(String, String)]) -> Result<(), SecurityError> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                // Split on commas and check the final encoding.
                let final_encoding = value.rsplit(',').next().map(str::trim).unwrap_or_default();

                if !final_encoding.eq_ignore_ascii_case("chunked") {
                    return Err(SecurityError::SmuggleTECL);
                }
            }
        }
        Ok(())
    }

    /// Run all applicable smuggling checks on the given headers.
    ///
    /// Set `is_h2_origin` to `true` when the request arrived via HTTP/2
    /// (enables the H2 downgrade check).
    ///
    /// # Errors
    ///
    /// Returns the first `SecurityError` encountered.
    pub fn check_all(
        headers: &[(String, String)],
        is_h2_origin: bool,
    ) -> Result<(), SecurityError> {
        Self::check_cl_te(headers)?;
        Self::check_te_cl(headers)?;
        Self::check_duplicate_cl(headers)?;
        if is_h2_origin {
            Self::check_h2_downgrade(headers, true)?;
        }
        Ok(())
    }

    /// Check for H2-to-H1 downgrade smuggling.
    ///
    /// RFC 9113 section 8.2.2: When translating HTTP/2 to HTTP/1.1, certain
    /// headers MUST NOT be present: `connection`, `transfer-encoding`,
    /// `keep-alive`, `upgrade`, and `proxy-connection`. Pseudo-headers
    /// (`:authority`, `:method`, etc.) must also not leak into H1.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::SmuggleH2Downgrade`] if `is_from_h2` is true
    /// and any prohibited header is found.
    pub fn check_h2_downgrade(
        headers: &[(String, String)],
        is_from_h2: bool,
    ) -> Result<(), SecurityError> {
        const PROHIBITED: &[&str] = &[
            "connection",
            "transfer-encoding",
            "keep-alive",
            "upgrade",
            "proxy-connection",
        ];

        if !is_from_h2 {
            return Ok(());
        }

        for (name, value) in headers {
            let lower = name.to_ascii_lowercase();

            // Reject prohibited hop-by-hop headers.
            if PROHIBITED.iter().any(|&p| p == lower) {
                return Err(SecurityError::SmuggleH2Downgrade);
            }

            // RFC 9113 section 8.2.2: TE header is prohibited in H2
            // EXCEPT when the value is exactly "trailers" (case-insensitive).
            if lower == "te" && !value.trim().eq_ignore_ascii_case("trailers") {
                return Err(SecurityError::SmuggleH2Downgrade);
            }

            // Reject pseudo-headers leaking into the translation.
            if lower.starts_with(':') {
                return Err(SecurityError::SmuggleH2Downgrade);
            }
        }

        Ok(())
    }
}
