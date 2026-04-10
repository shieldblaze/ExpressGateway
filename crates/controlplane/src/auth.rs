//! Session token authentication and per-node rate limiting.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use uuid::Uuid;

/// Manages session tokens for node authentication.
///
/// Workflow:
/// 1. A node registers with a shared `auth_token` (pre-shared secret).
/// 2. The control plane validates the auth_token and issues a `session_token` (UUID).
/// 3. Subsequent requests include the session_token in the `x-session-token` header.
/// 4. The control plane validates the session_token on each request.
pub struct SessionAuthenticator {
    /// The pre-shared auth token that nodes must present to register.
    auth_token: String,
    /// Map of session_token -> node_id for active sessions.
    sessions: DashMap<String, String>,
    /// Per-node rate limiter: node_id -> RateLimiterEntry.
    rate_limiters: DashMap<String, RateLimiterEntry>,
    /// Maximum requests per second per node.
    max_requests_per_sec: u64,
}

/// Rate limiter state for a single node.
struct RateLimiterEntry {
    count: AtomicU64,
    window_start: parking_lot::Mutex<Instant>,
}

impl RateLimiterEntry {
    fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
            window_start: parking_lot::Mutex::new(Instant::now()),
        }
    }
}

impl SessionAuthenticator {
    /// Create a new authenticator with the given pre-shared auth token.
    pub fn new(auth_token: String) -> Self {
        Self {
            auth_token,
            sessions: DashMap::new(),
            rate_limiters: DashMap::new(),
            max_requests_per_sec: 100,
        }
    }

    /// Create a new authenticator with a custom rate limit.
    pub fn with_rate_limit(auth_token: String, max_requests_per_sec: u64) -> Self {
        Self {
            auth_token,
            sessions: DashMap::new(),
            rate_limiters: DashMap::new(),
            max_requests_per_sec,
        }
    }

    /// Register a node by validating the auth_token and issuing a session_token.
    /// Returns `Some(session_token)` on success, `None` if the auth_token is invalid.
    pub fn register(&self, auth_token: &str, node_id: &str) -> Option<String> {
        if auth_token != self.auth_token {
            return None;
        }

        // Revoke any existing session for this node
        self.revoke_by_node(node_id);

        let session_token = Uuid::new_v4().to_string();
        self.sessions
            .insert(session_token.clone(), node_id.to_string());
        self.rate_limiters
            .entry(node_id.to_string())
            .or_insert_with(RateLimiterEntry::new);
        Some(session_token)
    }

    /// Validate a session token. Returns the associated node_id if valid.
    pub fn validate(&self, session_token: &str) -> Option<String> {
        self.sessions.get(session_token).map(|r| r.value().clone())
    }

    /// Validate a session token and check rate limiting.
    /// Returns `Ok(node_id)` if valid and within rate limit,
    /// `Err(AuthError::InvalidToken)` if the token is invalid,
    /// `Err(AuthError::RateLimited)` if the rate limit is exceeded.
    pub fn validate_and_rate_limit(&self, session_token: &str) -> Result<String, AuthError> {
        let node_id = self
            .validate(session_token)
            .ok_or(AuthError::InvalidToken)?;

        if !self.check_rate_limit(&node_id) {
            return Err(AuthError::RateLimited);
        }

        Ok(node_id)
    }

    /// Check and update the rate limiter for the given node.
    /// Returns true if the request is allowed.
    fn check_rate_limit(&self, node_id: &str) -> bool {
        let entry = self
            .rate_limiters
            .entry(node_id.to_string())
            .or_insert_with(RateLimiterEntry::new);

        let mut window_start = entry.window_start.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(*window_start);

        if elapsed.as_secs() >= 1 {
            // Reset the window
            *window_start = now;
            entry.count.store(1, Ordering::Relaxed);
            true
        } else {
            let count = entry.count.fetch_add(1, Ordering::Relaxed) + 1;
            count <= self.max_requests_per_sec
        }
    }

    /// Revoke all sessions for a given node.
    pub fn revoke_by_node(&self, node_id: &str) {
        self.sessions.retain(|_, v| v.as_str() != node_id);
    }

    /// Revoke a specific session token.
    pub fn revoke(&self, session_token: &str) {
        self.sessions.remove(session_token);
    }

    /// Get the number of active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }
}

/// Authentication errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("invalid session token")]
    InvalidToken,
    #[error("rate limit exceeded")]
    RateLimited,
}

/// Create a shared authenticator.
pub fn new_shared(auth_token: String) -> Arc<SessionAuthenticator> {
    Arc::new(SessionAuthenticator::new(auth_token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_with_valid_token() {
        let auth = SessionAuthenticator::new("secret".into());
        let token = auth.register("secret", "node-1");
        assert!(token.is_some());
        let token = token.unwrap();
        assert_eq!(auth.validate(&token), Some("node-1".to_string()));
    }

    #[test]
    fn register_with_invalid_token() {
        let auth = SessionAuthenticator::new("secret".into());
        let token = auth.register("wrong", "node-1");
        assert!(token.is_none());
    }

    #[test]
    fn validate_unknown_session() {
        let auth = SessionAuthenticator::new("secret".into());
        assert_eq!(auth.validate("nonexistent"), None);
    }

    #[test]
    fn revoke_session() {
        let auth = SessionAuthenticator::new("secret".into());
        let token = auth.register("secret", "node-1").unwrap();
        assert!(auth.validate(&token).is_some());
        auth.revoke(&token);
        assert!(auth.validate(&token).is_none());
    }

    #[test]
    fn revoke_by_node() {
        let auth = SessionAuthenticator::new("secret".into());
        let token = auth.register("secret", "node-1").unwrap();
        auth.revoke_by_node("node-1");
        assert!(auth.validate(&token).is_none());
    }

    #[test]
    fn re_register_revokes_old_session() {
        let auth = SessionAuthenticator::new("secret".into());
        let old_token = auth.register("secret", "node-1").unwrap();
        let new_token = auth.register("secret", "node-1").unwrap();
        assert!(auth.validate(&old_token).is_none());
        assert!(auth.validate(&new_token).is_some());
    }

    #[test]
    fn rate_limiting() {
        let auth = SessionAuthenticator::with_rate_limit("secret".into(), 5);
        let token = auth.register("secret", "node-1").unwrap();

        // First 5 should succeed
        for _ in 0..5 {
            assert!(auth.validate_and_rate_limit(&token).is_ok());
        }

        // 6th should be rate limited
        assert_eq!(
            auth.validate_and_rate_limit(&token),
            Err(AuthError::RateLimited)
        );
    }

    #[test]
    fn rate_limit_invalid_token() {
        let auth = SessionAuthenticator::new("secret".into());
        assert_eq!(
            auth.validate_and_rate_limit("bad"),
            Err(AuthError::InvalidToken)
        );
    }

    #[test]
    fn active_session_count() {
        let auth = SessionAuthenticator::new("secret".into());
        assert_eq!(auth.active_session_count(), 0);
        auth.register("secret", "node-1");
        assert_eq!(auth.active_session_count(), 1);
        auth.register("secret", "node-2");
        assert_eq!(auth.active_session_count(), 2);
        auth.revoke_by_node("node-1");
        assert_eq!(auth.active_session_count(), 1);
    }
}
