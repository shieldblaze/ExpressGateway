//! HTTP request smuggling detection: RFC 9112 §6.1 checks plus the H2→H1 downgrade rules of
//! RFC 9113 §8.2.2.

use crate::SecurityError;

/// Mode selector for [`SmuggleDetector::check_all_mode`], chosen per request from the protocol
/// version and `[runtime].strict_te` (SEC-2-15 matrix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SmuggleMode {
    /// RFC 9112 §6.1 checks; a final `chunked` codec suffices, so `gzip, chunked` passes.
    #[default]
    H1,

    /// H1 plus [`SmuggleDetector::check_te_strict`] — only the bare token `chunked` passes.
    H1Strict,

    /// H1 plus the RFC 9113 §8.2.2 H2→H1 downgrade check.
    H2,
}

/// Stateless detector for HTTP request smuggling attack patterns.
pub struct SmuggleDetector;

impl SmuggleDetector {
    /// Reject differing duplicate `Content-Length` headers (RFC 9110 §8.6 MUST).
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

    /// Reject CL-TE smuggling — both `Content-Length` and `Transfer-Encoding` (RFC 9112 §6.1).
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

    /// Reject TE-CL smuggling — `Transfer-Encoding` whose FINAL codec is not `chunked` (§6.1 MUST).
    pub fn check_te_cl(headers: &[(String, String)]) -> Result<(), SecurityError> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                let final_encoding = value.rsplit(',').next().map(str::trim).unwrap_or_default();

                if !final_encoding.eq_ignore_ascii_case("chunked") {
                    return Err(SecurityError::SmuggleTECL);
                }
            }
        }
        Ok(())
    }

    /// Run every applicable check, SHORT-CIRCUITING on the first failure; `is_h2_origin` adds H2→H1.
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

    /// Mode-aware [`check_all`](Self::check_all); short-circuits on the first failure.
    pub fn check_all_mode(
        headers: &[(String, String)],
        mode: SmuggleMode,
    ) -> Result<(), SecurityError> {
        Self::check_cl_te(headers)?;
        Self::check_te_cl(headers)?;
        Self::check_duplicate_cl(headers)?;
        match mode {
            SmuggleMode::H1 => {}
            SmuggleMode::H1Strict => {
                Self::check_te_strict(headers)?;
            }
            SmuggleMode::H2 => {
                Self::check_h2_downgrade(headers, true)?;
            }
        }
        Ok(())
    }

    /// Strict TE policy (SEC-2-15): only the bare token `chunked` passes. RFC 9112 §6.1 allows a
    /// codec chain ahead of `chunked`, but upstreams routinely mis-decode it and the still-gzipped
    /// payload becomes a body-length mismatch across the gateway. Errors reuse
    /// [`SecurityError::SmuggleTECL`] deliberately — a new variant is an API break.
    pub fn check_te_strict(headers: &[(String, String)]) -> Result<(), SecurityError> {
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("transfer-encoding") {
                for codec in value.split(',') {
                    let codec = codec.trim();
                    if codec.is_empty() {
                        // `chunked,` / `,chunked` — an empty codec is its own smell.
                        return Err(SecurityError::SmuggleTECL);
                    }
                    if !codec.eq_ignore_ascii_case("chunked") {
                        return Err(SecurityError::SmuggleTECL);
                    }
                }
            }
        }
        Ok(())
    }

    /// Reject H2→H1 downgrade smuggling (RFC 9113 §8.2.2): hop-by-hop headers, a `te` that is not
    /// exactly `trailers`, and pseudo-headers leaking into the translated H1 message.
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

            if PROHIBITED.iter().any(|&p| p == lower) {
                return Err(SecurityError::SmuggleH2Downgrade);
            }

            // `te` is legal in H2 ONLY as exactly `trailers`.
            if lower == "te" && !value.trim().eq_ignore_ascii_case("trailers") {
                return Err(SecurityError::SmuggleH2Downgrade);
            }

            // Pseudo-header leaking into the H1 translation.
            if lower.starts_with(':') {
                return Err(SecurityError::SmuggleH2Downgrade);
            }
        }

        Ok(())
    }
}
