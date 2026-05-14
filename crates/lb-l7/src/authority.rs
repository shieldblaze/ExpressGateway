//! ROUND8-L7-09 — protocol-neutral authority validator.
//!
//! References:
//! - HAProxy `BUG/MAJOR: http: forbid comma character in authority
//!   value`.
//! - HAProxy `BUG/MEDIUM: h1: Enforce the authority validation during
//!   H1 request parsing` (H1 parser missed the check that H2/H3 had).
//! - RFC 9110 §4 authority component definition.
//! - RFC 3986 §3.2 host = IP-literal / IPv4 / reg-name.
//!
//! Today the gateway validates SNI / Host agreement
//! (`sni_authority::check_sni_authority`) but does not sanitise the
//! authority value itself. A `,` / whitespace / control byte inside
//! the value can desync upstream routing decisions or punch through
//! a Host-based ACL. This validator runs on every parser path before
//! the agreement comparison.

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
    /// IPv6 bracket balance mismatch (`[` without `]`, or vice versa,
    /// or more than one of either).
    UnbalancedBrackets,
    /// Port suffix present but contained non-digit bytes.
    InvalidPort,
}

/// Validate an authority value against the canonical predicate.
///
/// Returns `Ok(())` if the value passes; `Err(AuthorityError)`
/// otherwise. Callers MUST run this before any agreement comparison
/// (Host vs `:authority`, SNI vs Host).
///
/// # Errors
///
/// Returns [`AuthorityError`] when the value fails any of the
/// predicates documented on the variants.
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
    // IPv6 bracket balance: RFC 3986 §3.2.2 `IP-literal = "["
    // (IPv6address / IPvFuture) "]"`. Exactly one bracket pair
    // allowed; the unbracketed reg-name form has zero.
    let opens = value.bytes().filter(|&b| b == b'[').count();
    let closes = value.bytes().filter(|&b| b == b']').count();
    if opens != closes || opens > 1 {
        return Err(AuthorityError::UnbalancedBrackets);
    }
    // Port suffix validation: if a `:` appears after the host
    // portion, every byte after the LAST `:` must be a digit.
    // (IPv6 addresses contain colons inside the brackets; only the
    // colon after `]` is the port separator.)
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

/// Return the port suffix (the substring after the last `:` that is
/// not inside brackets), or `None` if no port is present.
fn port_suffix(value: &str) -> Option<&str> {
    // If brackets are present, the port (if any) is what's after `]:`.
    if let Some(rb) = value.rfind(']') {
        let after = value.get(rb + 1..)?;
        return after.strip_prefix(':');
    }
    // No brackets — port is after the LAST `:`, but only if no other
    // colons appear (no raw IPv6 outside brackets, per RFC 3986).
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
        // `::1` (no brackets) has multiple colons. Our heuristic
        // skips port validation when colon count > 1; the RFC 3986
        // grammar says this is invalid as an authority, but a more
        // expensive grammar parse is out of scope for this fix.
        // Pinning the current behaviour so any future tightening
        // surfaces here (e.g. via the `http::uri` authority parser).
        assert!(validate("::1").is_ok());
    }
}
