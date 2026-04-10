//! HTTP/2 CONNECT tunneling per RFC 9113 Section 8.5.
//!
//! In HTTP/2, the CONNECT method establishes a tunnel to the destination
//! indicated by the `:authority` pseudo-header. Only `:method` and `:authority`
//! pseudo-headers are allowed; `:scheme` and `:path` must be absent.
//! After the tunnel is established, bidirectional byte forwarding begins.

use std::net::SocketAddr;

/// A validated CONNECT request.
#[derive(Debug, Clone)]
pub struct ConnectRequest {
    /// The target authority (host:port) for the tunnel.
    pub authority: String,
    /// Parsed target address (if the authority is a valid socket address).
    pub target_addr: Option<SocketAddr>,
}

/// Errors that can occur when validating a CONNECT request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    /// The `:authority` pseudo-header is missing.
    #[error("CONNECT request missing :authority")]
    MissingAuthority,

    /// The `:scheme` pseudo-header is present (not allowed for CONNECT).
    #[error("CONNECT request must not include :scheme")]
    SchemePresent,

    /// The `:path` pseudo-header is present (not allowed for CONNECT).
    #[error("CONNECT request must not include :path")]
    PathPresent,
}

/// Validate an HTTP/2 CONNECT request.
///
/// Per RFC 9113 Section 8.5:
/// - Only `:method` and `:authority` pseudo-headers are allowed.
/// - `:scheme` and `:path` must not be present.
pub fn validate_connect(
    authority: Option<&str>,
    has_scheme: bool,
    has_path: bool,
) -> Result<ConnectRequest, ConnectError> {
    if has_scheme {
        return Err(ConnectError::SchemePresent);
    }
    if has_path {
        return Err(ConnectError::PathPresent);
    }

    let authority = authority
        .filter(|a| !a.is_empty())
        .ok_or(ConnectError::MissingAuthority)?;

    // Try to parse as a socket address.
    let target_addr = authority.parse::<SocketAddr>().ok();

    Ok(ConnectRequest {
        authority: authority.to_string(),
        target_addr,
    })
}

/// State of a CONNECT tunnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelState {
    /// The tunnel is being established (CONNECT sent, awaiting 2xx).
    Connecting,
    /// The tunnel is open, bidirectional byte forwarding is active.
    Open,
    /// The tunnel has been closed.
    Closed,
}

/// Metrics for a CONNECT tunnel.
#[derive(Debug, Clone, Copy, Default)]
pub struct TunnelMetrics {
    /// Bytes sent from client to target.
    pub bytes_upstream: u64,
    /// Bytes sent from target to client.
    pub bytes_downstream: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_connect_with_authority() {
        let result = validate_connect(Some("example.com:443"), false, false).unwrap();
        assert_eq!(result.authority, "example.com:443");
        assert!(result.target_addr.is_none()); // Domain name, not socket addr.
    }

    #[test]
    fn valid_connect_with_socket_addr() {
        let result = validate_connect(Some("127.0.0.1:8080"), false, false).unwrap();
        assert_eq!(result.target_addr, Some("127.0.0.1:8080".parse().unwrap()));
    }

    #[test]
    fn connect_missing_authority() {
        let result = validate_connect(None, false, false);
        assert_eq!(result.unwrap_err(), ConnectError::MissingAuthority);
    }

    #[test]
    fn connect_empty_authority() {
        let result = validate_connect(Some(""), false, false);
        assert_eq!(result.unwrap_err(), ConnectError::MissingAuthority);
    }

    #[test]
    fn connect_with_scheme_rejected() {
        let result = validate_connect(Some("example.com:443"), true, false);
        assert_eq!(result.unwrap_err(), ConnectError::SchemePresent);
    }

    #[test]
    fn connect_with_path_rejected() {
        let result = validate_connect(Some("example.com:443"), false, true);
        assert_eq!(result.unwrap_err(), ConnectError::PathPresent);
    }

    #[test]
    fn tunnel_state_transitions() {
        let state = TunnelState::Connecting;
        assert_eq!(state, TunnelState::Connecting);
        let state = TunnelState::Open;
        assert_eq!(state, TunnelState::Open);
        let state = TunnelState::Closed;
        assert_eq!(state, TunnelState::Closed);
    }
}
