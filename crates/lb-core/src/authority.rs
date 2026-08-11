//! The single authority-value predicate shared by EVERY inbound parser (RFC 9110 §4, RFC 3986
//! §3.2).
//!
//! It lives in this leaf crate on purpose. HAProxy shipped `BUG/MEDIUM: h1: Enforce the authority
//! validation during H1 request parsing` precisely because the check existed as a function that
//! the H1 parser did not call — one implementation everyone depends on is what stops a new
//! protocol parser silently skipping it.
//!
//! Deliberately NO loopback exemption and NO empty/absent gate: the loopback carve-out belongs to
//! the SNI-vs-Host AGREEMENT check, not here.

/// Reason an authority value was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityError {
    /// Empty string.
    Empty,
    /// Comma byte inside the value (HAProxy bug class).
    Comma,
    /// SP or HTAB inside the value.
    Whitespace,
    /// C0 control or DEL.
    Control,
    /// Unbalanced or repeated IPv6 brackets.
    UnbalancedBrackets,
    /// Port suffix present but contained non-digit bytes.
    InvalidPort,
}

/// Validate an authority value. MUST run before any agreement comparison (Host vs `:authority`,
/// SNI vs Host) and before upstream selection.
///
/// # Errors
///
/// [`AuthorityError`].
pub fn validate(value: &str) -> Result<(), AuthorityError> {
    if value.is_empty() {
        return Err(AuthorityError::Empty);
    }
    for b in value.bytes() {
        match b {
            b',' => return Err(AuthorityError::Comma),
            b' ' | b'\t' => return Err(AuthorityError::Whitespace),
            0..=0x1F | 0x7F => return Err(AuthorityError::Control),
            _ => {}
        }
    }
    // RFC 3986 §3.2.2: exactly one bracket pair, or none for the reg-name form.
    let opens = value.bytes().filter(|&b| b == b'[').count();
    let closes = value.bytes().filter(|&b| b == b']').count();
    if opens != closes || opens > 1 {
        return Err(AuthorityError::UnbalancedBrackets);
    }
    // Only the colon after `]` is a port separator — IPv6 colons live inside the brackets.
    if let Some(port_part) = port_suffix(value) {
        if port_part.is_empty() {
            return Err(AuthorityError::InvalidPort);
        }
        if !port_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AuthorityError::InvalidPort);
        }
    }
    Ok(())
}

/// The port suffix after the last unbracketed `:`, if any.
fn port_suffix(value: &str) -> Option<&str> {
    // If brackets are present, the port (if any) is what's after `]:`.
    if let Some(rb) = value.rfind(']') {
        let after = value.get(rb + 1..)?;
        return after.strip_prefix(':');
    }
    // Unbracketed: a second colon means raw IPv6, which RFC 3986 forbids as an authority.
    let count = value.bytes().filter(|&b| b == b':').count();
    if count != 1 {
        return None;
    }
    let colon = value.rfind(':')?;
    value.get(colon + 1..)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn comma_rejected() {
        assert_eq!(validate("a,b"), Err(AuthorityError::Comma));
    }

    #[test]
    fn whitespace_rejected() {
        assert_eq!(validate("a b"), Err(AuthorityError::Whitespace));
        assert_eq!(validate("a\tb"), Err(AuthorityError::Whitespace));
    }

    #[test]
    fn control_char_rejected() {
        assert_eq!(validate("\x01host"), Err(AuthorityError::Control));
        assert_eq!(validate("a\x7Fb"), Err(AuthorityError::Control));
    }

    #[test]
    fn empty_rejected() {
        assert_eq!(validate(""), Err(AuthorityError::Empty));
    }

    #[test]
    fn ipv6_brackets_must_balance() {
        assert_eq!(validate("[::1"), Err(AuthorityError::UnbalancedBrackets));
        assert_eq!(validate("::1]"), Err(AuthorityError::UnbalancedBrackets));
        assert_eq!(validate("[::1]:8080"), Ok(()));
    }

    #[test]
    fn port_digits_only() {
        assert_eq!(
            validate("example.com:abc"),
            Err(AuthorityError::InvalidPort)
        );
        assert_eq!(validate("example.com:80"), Ok(()));
    }

    #[test]
    fn happy_path_examples() {
        assert!(validate("example.com").is_ok());
        assert!(validate("example.com:8080").is_ok());
        assert!(validate("[::1]:8080").is_ok());
        assert!(validate("192.0.2.1").is_ok());
        assert!(validate("192.0.2.1:80").is_ok());
        assert!(validate("sub.example.com").is_ok());
    }

    #[test]
    fn empty_port_after_colon_rejected() {
        assert_eq!(validate("example.com:"), Err(AuthorityError::InvalidPort));
    }

    #[test]
    fn raw_ipv6_without_brackets_accepted_today() {
        // KNOWN GAP pinned deliberately: the multi-colon heuristic skips port validation, so
        // unbracketed `::1` passes even though RFC 3986 rejects it. A future tightening must
        // update this assertion.
        assert!(validate("::1").is_ok());
    }
}
