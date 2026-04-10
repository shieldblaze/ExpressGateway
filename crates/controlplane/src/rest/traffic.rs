//! Traffic policy management endpoints.
//!
//! Manages per-cluster traffic policies including:
//! - Backend weights (for weighted load balancing)
//! - Circuit breaker configuration
//! - Rate limit configuration

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
};
use serde::{Deserialize, Serialize};

use super::AppState;

/// Traffic policy for a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficPolicy {
    pub cluster_id: String,
    pub weights: Option<WeightPolicy>,
    pub circuit_breaker: Option<CircuitBreakerPolicy>,
    pub rate_limit: Option<RateLimitPolicy>,
}

/// Weighted traffic distribution across backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightPolicy {
    /// Map of backend address -> weight (0-1000).
    pub backend_weights: std::collections::HashMap<String, u32>,
}

/// Circuit breaker configuration for a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerPolicy {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Duration in seconds to keep the circuit open before half-open.
    pub open_duration_s: u64,
    /// Number of successes in half-open state to close the circuit.
    pub success_threshold: u32,
    /// Maximum concurrent requests (0 = unlimited).
    pub max_concurrent_requests: u64,
}

/// Rate limit configuration for a cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    /// Maximum requests per second.
    pub requests_per_second: u64,
    /// Burst capacity (token bucket size).
    pub burst: u64,
    /// Whether to apply per-client or globally.
    pub scope: RateLimitScope,
}

/// Scope of rate limiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitScope {
    /// Rate limit per source IP / client.
    PerClient,
    /// Global rate limit across all clients.
    Global,
}

/// Request body for updating traffic policy.
#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTrafficPolicyRequest {
    pub weights: Option<WeightPolicy>,
    pub circuit_breaker: Option<CircuitBreakerPolicy>,
    pub rate_limit: Option<RateLimitPolicy>,
}

/// Build traffic policy routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/clusters/{id}/traffic-policy", get(get_traffic_policy))
        .route("/clusters/{id}/traffic-policy", put(update_traffic_policy))
        .route(
            "/clusters/{id}/traffic-policy/weights",
            put(update_weights),
        )
        .route(
            "/clusters/{id}/traffic-policy/circuit-breaker",
            put(update_circuit_breaker),
        )
        .route(
            "/clusters/{id}/traffic-policy/rate-limit",
            put(update_rate_limit),
        )
}

/// GET /clusters/:id/traffic-policy - Get traffic policy for a cluster.
async fn get_traffic_policy(
    State(state): State<AppState>,
    Path(cluster_id): Path<String>,
) -> Result<Json<TrafficPolicy>, StatusCode> {
    // Verify the cluster exists.
    if !state.store.clusters.contains_key(&cluster_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    let policy = state
        .store
        .traffic_policies
        .get(&cluster_id)
        .map(|r| r.value().clone())
        .unwrap_or_else(|| TrafficPolicy {
            cluster_id: cluster_id.clone(),
            weights: None,
            circuit_breaker: None,
            rate_limit: None,
        });

    Ok(Json(policy))
}

/// PUT /clusters/:id/traffic-policy - Update full traffic policy for a cluster.
async fn update_traffic_policy(
    State(state): State<AppState>,
    Path(cluster_id): Path<String>,
    Json(req): Json<UpdateTrafficPolicyRequest>,
) -> Result<Json<TrafficPolicy>, StatusCode> {
    if !state.store.clusters.contains_key(&cluster_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // Validate weights if provided.
    if let Some(ref weights) = req.weights {
        for &w in weights.backend_weights.values() {
            if w > 1000 {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }

    let policy = TrafficPolicy {
        cluster_id: cluster_id.clone(),
        weights: req.weights,
        circuit_breaker: req.circuit_breaker,
        rate_limit: req.rate_limit,
    };

    state
        .store
        .traffic_policies
        .insert(cluster_id, policy.clone());
    Ok(Json(policy))
}

/// PUT /clusters/:id/traffic-policy/weights - Update only the weight policy.
async fn update_weights(
    State(state): State<AppState>,
    Path(cluster_id): Path<String>,
    Json(weights): Json<WeightPolicy>,
) -> Result<Json<TrafficPolicy>, StatusCode> {
    if !state.store.clusters.contains_key(&cluster_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    for &w in weights.backend_weights.values() {
        if w > 1000 {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let mut policy = state
        .store
        .traffic_policies
        .entry(cluster_id.clone())
        .or_insert_with(|| TrafficPolicy {
            cluster_id: cluster_id.clone(),
            weights: None,
            circuit_breaker: None,
            rate_limit: None,
        });

    policy.weights = Some(weights);
    Ok(Json(policy.value().clone()))
}

/// PUT /clusters/:id/traffic-policy/circuit-breaker - Update circuit breaker.
async fn update_circuit_breaker(
    State(state): State<AppState>,
    Path(cluster_id): Path<String>,
    Json(cb): Json<CircuitBreakerPolicy>,
) -> Result<Json<TrafficPolicy>, StatusCode> {
    if !state.store.clusters.contains_key(&cluster_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    if cb.failure_threshold == 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut policy = state
        .store
        .traffic_policies
        .entry(cluster_id.clone())
        .or_insert_with(|| TrafficPolicy {
            cluster_id: cluster_id.clone(),
            weights: None,
            circuit_breaker: None,
            rate_limit: None,
        });

    policy.circuit_breaker = Some(cb);
    Ok(Json(policy.value().clone()))
}

/// PUT /clusters/:id/traffic-policy/rate-limit - Update rate limit.
async fn update_rate_limit(
    State(state): State<AppState>,
    Path(cluster_id): Path<String>,
    Json(rl): Json<RateLimitPolicy>,
) -> Result<Json<TrafficPolicy>, StatusCode> {
    if !state.store.clusters.contains_key(&cluster_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    if rl.requests_per_second == 0 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let mut policy = state
        .store
        .traffic_policies
        .entry(cluster_id.clone())
        .or_insert_with(|| TrafficPolicy {
            cluster_id: cluster_id.clone(),
            weights: None,
            circuit_breaker: None,
            rate_limit: None,
        });

    policy.rate_limit = Some(rl);
    Ok(Json(policy.value().clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rest;
    use crate::store::{ClusterEntry, ControlPlaneStore};
    use axum::body::Body;
    use axum::http::Request;
    use chrono::Utc;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app_with_cluster() -> (Router, Arc<ControlPlaneStore>, String) {
        let store = ControlPlaneStore::new_shared();
        let now = Utc::now();
        let cluster_id = "test-cluster".to_string();
        store.clusters.insert(
            cluster_id.clone(),
            ClusterEntry {
                id: cluster_id.clone(),
                name: "test".into(),
                lb_strategy: "round_robin".into(),
                max_connections_per_node: None,
                drain_timeout_s: None,
                nodes: vec![],
                created_at: now,
                updated_at: now,
            },
        );
        let app = rest::router(store.clone());
        (app, store, cluster_id)
    }

    #[tokio::test]
    async fn get_default_traffic_policy() {
        let (app, _store, cluster_id) = test_app_with_cluster();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy",
                        cluster_id
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let policy: TrafficPolicy = serde_json::from_slice(&body).unwrap();
        assert!(policy.weights.is_none());
        assert!(policy.circuit_breaker.is_none());
        assert!(policy.rate_limit.is_none());
    }

    #[tokio::test]
    async fn update_full_traffic_policy() {
        let (app, _store, cluster_id) = test_app_with_cluster();

        let mut backend_weights = std::collections::HashMap::new();
        backend_weights.insert("10.0.0.1:8080".into(), 500u32);
        backend_weights.insert("10.0.0.2:8080".into(), 500u32);

        let req_body = serde_json::to_string(&UpdateTrafficPolicyRequest {
            weights: Some(WeightPolicy { backend_weights }),
            circuit_breaker: Some(CircuitBreakerPolicy {
                failure_threshold: 5,
                open_duration_s: 30,
                success_threshold: 3,
                max_concurrent_requests: 100,
            }),
            rate_limit: Some(RateLimitPolicy {
                requests_per_second: 1000,
                burst: 50,
                scope: RateLimitScope::Global,
            }),
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy",
                        cluster_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let policy: TrafficPolicy = serde_json::from_slice(&body).unwrap();
        assert!(policy.weights.is_some());
        assert!(policy.circuit_breaker.is_some());
        assert!(policy.rate_limit.is_some());
    }

    #[tokio::test]
    async fn update_weights_only() {
        let (app, _store, cluster_id) = test_app_with_cluster();

        let mut backend_weights = std::collections::HashMap::new();
        backend_weights.insert("10.0.0.1:8080".into(), 700u32);
        backend_weights.insert("10.0.0.2:8080".into(), 300u32);

        let req_body = serde_json::to_string(&WeightPolicy { backend_weights }).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy/weights",
                        cluster_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_weights_invalid_value() {
        let (app, _store, cluster_id) = test_app_with_cluster();

        let mut backend_weights = std::collections::HashMap::new();
        backend_weights.insert("10.0.0.1:8080".into(), 2000u32); // > 1000

        let req_body = serde_json::to_string(&WeightPolicy { backend_weights }).unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy/weights",
                        cluster_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn traffic_policy_cluster_not_found() {
        let store = ControlPlaneStore::new_shared();
        let app = rest::router(store);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/controlplane/clusters/nonexistent/traffic-policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_circuit_breaker_zero_threshold() {
        let (app, _store, cluster_id) = test_app_with_cluster();

        let req_body = serde_json::to_string(&CircuitBreakerPolicy {
            failure_threshold: 0,
            open_duration_s: 30,
            success_threshold: 3,
            max_concurrent_requests: 100,
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy/circuit-breaker",
                        cluster_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_rate_limit_zero_rps() {
        let (app, _store, cluster_id) = test_app_with_cluster();

        let req_body = serde_json::to_string(&RateLimitPolicy {
            requests_per_second: 0,
            burst: 10,
            scope: RateLimitScope::Global,
        })
        .unwrap();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!(
                        "/api/v1/controlplane/clusters/{}/traffic-policy/rate-limit",
                        cluster_id
                    ))
                    .header("content-type", "application/json")
                    .body(Body::from(req_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
