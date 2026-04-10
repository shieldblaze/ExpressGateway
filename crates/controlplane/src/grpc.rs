//! gRPC service implementations for the control plane.
//!
//! Since proto files are not yet set up with tonic-build, these use manual
//! message types and service trait definitions. The service implementations
//! are fully functional against the in-memory store.
//!
//! When proto files are available, the message types below should be replaced
//! with generated code and the service impls adapted to implement the
//! generated server traits.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::auth::SessionAuthenticator;
use crate::node_registry::NodeEntry;
use crate::reconnect_protection::ReconnectProtection;
use crate::store::ControlPlaneStore;

// ---------------------------------------------------------------------------
// Common message types (manual definitions, no protobuf)
// ---------------------------------------------------------------------------

/// Node registration request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNodeRequest {
    pub node_id: String,
    pub address: String,
    pub auth_token: String,
}

/// Node registration response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNodeResponse {
    pub session_token: String,
    pub heartbeat_interval_s: u64,
}

/// Heartbeat request from a data plane node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    pub node_id: String,
    pub session_token: String,
}

/// Heartbeat response to a data plane node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    pub acknowledged: bool,
}

/// Configuration fetch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfigRequest {
    pub node_id: String,
    pub session_token: String,
    /// If set, only return config if newer than this version.
    pub last_known_version: Option<u64>,
}

/// Configuration fetch response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfigResponse {
    pub version: u64,
    pub config_json: String,
    /// True if the config has changed since last_known_version.
    pub changed: bool,
}

/// Node control command (drain, undrain, disconnect).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeControlRequest {
    pub node_id: String,
    pub command: String,
}

/// Node control response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeControlResponse {
    pub success: bool,
    pub message: String,
}

/// Health status report from an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReportRequest {
    pub node_id: String,
    pub session_token: String,
    /// Per-backend health status: backend_id -> healthy.
    pub backend_statuses: std::collections::HashMap<String, bool>,
}

/// Health status report response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReportResponse {
    pub acknowledged: bool,
}

// ---------------------------------------------------------------------------
// gRPC service errors
// ---------------------------------------------------------------------------

/// Errors returned by gRPC service methods.
#[derive(Debug, thiserror::Error)]
pub enum GrpcServiceError {
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("rate limited")]
    RateLimited,

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl From<GrpcServiceError> for tonic::Status {
    fn from(e: GrpcServiceError) -> Self {
        match e {
            GrpcServiceError::Unauthenticated(msg) => tonic::Status::unauthenticated(msg),
            GrpcServiceError::NotFound(msg) => tonic::Status::not_found(msg),
            GrpcServiceError::RateLimited => {
                tonic::Status::resource_exhausted("rate limit exceeded")
            }
            GrpcServiceError::InvalidArgument(msg) => tonic::Status::invalid_argument(msg),
            GrpcServiceError::Internal(msg) => tonic::Status::internal(msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Service trait definitions (to be implemented with tonic later)
// ---------------------------------------------------------------------------

/// Node registration and heartbeat service.
///
/// Proto equivalent:
/// ```proto
/// service NodeRegistrationService {
///   rpc Register(RegisterNodeRequest) returns (RegisterNodeResponse);
///   rpc Heartbeat(HeartbeatRequest) returns (HeartbeatResponse);
/// }
/// ```
pub trait NodeRegistrationService: Send + Sync {
    /// Register a data plane node with the control plane.
    fn register(
        &self,
        request: RegisterNodeRequest,
    ) -> impl std::future::Future<Output = Result<RegisterNodeResponse, tonic::Status>> + Send;

    /// Send a heartbeat from a data plane node.
    fn heartbeat(
        &self,
        request: HeartbeatRequest,
    ) -> impl std::future::Future<Output = Result<HeartbeatResponse, tonic::Status>> + Send;
}

/// Configuration distribution service.
///
/// Proto equivalent:
/// ```proto
/// service ConfigDistributionService {
///   rpc FetchConfig(FetchConfigRequest) returns (FetchConfigResponse);
/// }
/// ```
pub trait ConfigDistributionService: Send + Sync {
    /// Fetch the current configuration for a node.
    fn fetch_config(
        &self,
        request: FetchConfigRequest,
    ) -> impl std::future::Future<Output = Result<FetchConfigResponse, tonic::Status>> + Send;
}

/// Node control service for administrative actions.
///
/// Proto equivalent:
/// ```proto
/// service NodeControlService {
///   rpc Control(NodeControlRequest) returns (NodeControlResponse);
/// }
/// ```
pub trait NodeControlService: Send + Sync {
    /// Send a control command to a node (drain, undrain, disconnect).
    fn control(
        &self,
        request: NodeControlRequest,
    ) -> impl std::future::Future<Output = Result<NodeControlResponse, tonic::Status>> + Send;
}

/// Health aggregation service.
///
/// Proto equivalent:
/// ```proto
/// service HealthAggregationService {
///   rpc ReportHealth(HealthReportRequest) returns (HealthReportResponse);
/// }
/// ```
pub trait HealthAggregationService: Send + Sync {
    /// Report health status from an agent node.
    fn report_health(
        &self,
        request: HealthReportRequest,
    ) -> impl std::future::Future<Output = Result<HealthReportResponse, tonic::Status>> + Send;
}

// ---------------------------------------------------------------------------
// Concrete implementation against in-memory store
// ---------------------------------------------------------------------------

/// Shared state for gRPC service implementations.
pub struct GrpcServices {
    pub store: Arc<ControlPlaneStore>,
    pub authenticator: Arc<SessionAuthenticator>,
    pub reconnect_protection: Arc<ReconnectProtection>,
    /// Heartbeat interval to communicate to agents (seconds).
    pub heartbeat_interval_s: u64,
}

impl GrpcServices {
    pub fn new(
        store: Arc<ControlPlaneStore>,
        authenticator: Arc<SessionAuthenticator>,
        reconnect_protection: Arc<ReconnectProtection>,
    ) -> Self {
        Self {
            store,
            authenticator,
            reconnect_protection,
            heartbeat_interval_s: 10,
        }
    }

    /// Validate a session token and return the node ID.
    fn validate_session(&self, session_token: &str) -> Result<String, GrpcServiceError> {
        match self.authenticator.validate_and_rate_limit(session_token) {
            Ok(node_id) => Ok(node_id),
            Err(crate::auth::AuthError::InvalidToken) => Err(GrpcServiceError::Unauthenticated(
                "invalid session token".into(),
            )),
            Err(crate::auth::AuthError::RateLimited) => Err(GrpcServiceError::RateLimited),
        }
    }
}

impl NodeRegistrationService for GrpcServices {
    async fn register(
        &self,
        request: RegisterNodeRequest,
    ) -> Result<RegisterNodeResponse, tonic::Status> {
        if request.node_id.is_empty() {
            return Err(GrpcServiceError::InvalidArgument("node_id is required".into()).into());
        }
        if request.address.is_empty() {
            return Err(GrpcServiceError::InvalidArgument("address is required".into()).into());
        }

        // Rate-limit registration attempts (reconnect storm protection).
        if !self.reconnect_protection.try_acquire() {
            return Err(GrpcServiceError::RateLimited.into());
        }

        // Authenticate with the pre-shared token.
        let session_token = self
            .authenticator
            .register(&request.auth_token, &request.node_id)
            .ok_or_else(|| {
                GrpcServiceError::Unauthenticated("invalid auth token".into())
            })?;

        // Register in the node registry.
        let node = NodeEntry::new(
            request.node_id.clone(),
            request.address,
            Some(session_token.clone()),
        );
        self.store.nodes.insert(request.node_id, node);

        tracing::info!(session_token = %session_token, "node registered via gRPC");

        Ok(RegisterNodeResponse {
            session_token,
            heartbeat_interval_s: self.heartbeat_interval_s,
        })
    }

    async fn heartbeat(
        &self,
        request: HeartbeatRequest,
    ) -> Result<HeartbeatResponse, tonic::Status> {
        let node_id = self
            .validate_session(&request.session_token)
            .map_err(tonic::Status::from)?;

        // Verify the session matches the claimed node_id.
        if node_id != request.node_id {
            return Err(GrpcServiceError::Unauthenticated(
                "session token does not match node_id".into(),
            )
            .into());
        }

        // Update heartbeat in the registry.
        if let Some(mut entry) = self.store.nodes.get_mut(&node_id) {
            entry.record_heartbeat();
        } else {
            return Err(GrpcServiceError::NotFound(format!("node {} not found", node_id)).into());
        }

        Ok(HeartbeatResponse { acknowledged: true })
    }
}

impl ConfigDistributionService for GrpcServices {
    async fn fetch_config(
        &self,
        request: FetchConfigRequest,
    ) -> Result<FetchConfigResponse, tonic::Status> {
        self.validate_session(&request.session_token)
            .map_err(tonic::Status::from)?;

        let current = self.store.config_versions.current();

        match current {
            Some(version) => {
                let changed = request
                    .last_known_version
                    .map(|lkv| version.version > lkv)
                    .unwrap_or(true);

                let config_json = if changed {
                    serde_json::to_string(&version.snapshot).map_err(|e| {
                        tonic::Status::internal(format!("failed to serialize config: {}", e))
                    })?
                } else {
                    String::new()
                };

                Ok(FetchConfigResponse {
                    version: version.version,
                    config_json,
                    changed,
                })
            }
            None => Ok(FetchConfigResponse {
                version: 0,
                config_json: "{}".into(),
                changed: true,
            }),
        }
    }
}

impl NodeControlService for GrpcServices {
    async fn control(
        &self,
        request: NodeControlRequest,
    ) -> Result<NodeControlResponse, tonic::Status> {
        let mut entry = self
            .store
            .nodes
            .get_mut(&request.node_id)
            .ok_or_else(|| {
                GrpcServiceError::NotFound(format!("node {} not found", request.node_id))
            })
            .map_err(tonic::Status::from)?;

        let (success, message) = match request.command.as_str() {
            "drain" => {
                if entry.drain() {
                    (true, "node draining".into())
                } else {
                    (false, format!("cannot drain node in state {}", entry.state))
                }
            }
            "undrain" => {
                if entry.undrain() {
                    (true, "node undraining".into())
                } else {
                    (
                        false,
                        format!("cannot undrain node in state {}", entry.state),
                    )
                }
            }
            "disconnect" => {
                if entry.disconnect() {
                    (true, "node disconnected".into())
                } else {
                    (false, "node already disconnected".into())
                }
            }
            other => {
                return Err(
                    GrpcServiceError::InvalidArgument(format!("unknown command: {}", other)).into(),
                );
            }
        };

        Ok(NodeControlResponse { success, message })
    }
}

impl HealthAggregationService for GrpcServices {
    async fn report_health(
        &self,
        request: HealthReportRequest,
    ) -> Result<HealthReportResponse, tonic::Status> {
        self.validate_session(&request.session_token)
            .map_err(tonic::Status::from)?;

        tracing::debug!(
            node_id = %request.node_id,
            backends = ?request.backend_statuses.len(),
            "health report received"
        );

        // Store health reports -- in a full implementation this would update
        // per-backend health status in the store and potentially trigger
        // routing changes.
        // For now, we acknowledge receipt.

        Ok(HealthReportResponse {
            acknowledged: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_services() -> GrpcServices {
        let store = ControlPlaneStore::new_shared();
        let auth = Arc::new(SessionAuthenticator::new("test-secret".into()));
        let rp = Arc::new(ReconnectProtection::default_production());
        GrpcServices::new(store, auth, rp)
    }

    #[tokio::test]
    async fn register_node_success() {
        let svc = test_services();
        let resp = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        assert!(!resp.session_token.is_empty());
        assert_eq!(resp.heartbeat_interval_s, 10);
        assert!(svc.store.nodes.contains_key("node-1"));
    }

    #[tokio::test]
    async fn register_node_bad_auth() {
        let svc = test_services();
        let result = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "wrong".into(),
            })
            .await;

        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn register_node_empty_id() {
        let svc = test_services();
        let result = svc
            .register(RegisterNodeRequest {
                node_id: "".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn heartbeat_success() {
        let svc = test_services();
        let reg = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        let resp = svc
            .heartbeat(HeartbeatRequest {
                node_id: "node-1".into(),
                session_token: reg.session_token,
            })
            .await
            .unwrap();

        assert!(resp.acknowledged);
    }

    #[tokio::test]
    async fn heartbeat_bad_token() {
        let svc = test_services();
        let result = svc
            .heartbeat(HeartbeatRequest {
                node_id: "node-1".into(),
                session_token: "bad-token".into(),
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn heartbeat_wrong_node_id() {
        let svc = test_services();
        let reg = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        let result = svc
            .heartbeat(HeartbeatRequest {
                node_id: "node-2".into(),
                session_token: reg.session_token,
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn fetch_config_empty() {
        let svc = test_services();
        let reg = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        let resp = svc
            .fetch_config(FetchConfigRequest {
                node_id: "node-1".into(),
                session_token: reg.session_token,
                last_known_version: None,
            })
            .await
            .unwrap();

        assert_eq!(resp.version, 0);
        assert!(resp.changed);
    }

    #[tokio::test]
    async fn fetch_config_with_version() {
        let svc = test_services();
        svc.store
            .config_versions
            .push("v1".into(), serde_json::json!({"clusters": []}));

        let reg = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        // First fetch: changed=true
        let resp = svc
            .fetch_config(FetchConfigRequest {
                node_id: "node-1".into(),
                session_token: reg.session_token.clone(),
                last_known_version: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.version, 1);
        assert!(resp.changed);

        // Second fetch with same version: changed=false
        let resp = svc
            .fetch_config(FetchConfigRequest {
                node_id: "node-1".into(),
                session_token: reg.session_token,
                last_known_version: Some(1),
            })
            .await
            .unwrap();
        assert!(!resp.changed);
        assert!(resp.config_json.is_empty());
    }

    #[tokio::test]
    async fn control_drain_undrain() {
        let svc = test_services();
        svc.store.nodes.insert(
            "node-1".into(),
            NodeEntry::new("node-1".into(), "10.0.0.1:8080".into(), None),
        );

        let resp = svc
            .control(NodeControlRequest {
                node_id: "node-1".into(),
                command: "drain".into(),
            })
            .await
            .unwrap();
        assert!(resp.success);

        let resp = svc
            .control(NodeControlRequest {
                node_id: "node-1".into(),
                command: "undrain".into(),
            })
            .await
            .unwrap();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn control_unknown_command() {
        let svc = test_services();
        svc.store.nodes.insert(
            "node-1".into(),
            NodeEntry::new("node-1".into(), "10.0.0.1:8080".into(), None),
        );

        let result = svc
            .control(NodeControlRequest {
                node_id: "node-1".into(),
                command: "explode".into(),
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn control_node_not_found() {
        let svc = test_services();
        let result = svc
            .control(NodeControlRequest {
                node_id: "ghost".into(),
                command: "drain".into(),
            })
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn health_report() {
        let svc = test_services();
        let reg = svc
            .register(RegisterNodeRequest {
                node_id: "node-1".into(),
                address: "10.0.0.1:8080".into(),
                auth_token: "test-secret".into(),
            })
            .await
            .unwrap();

        let mut statuses = std::collections::HashMap::new();
        statuses.insert("backend-1".into(), true);
        statuses.insert("backend-2".into(), false);

        let resp = svc
            .report_health(HealthReportRequest {
                node_id: "node-1".into(),
                session_token: reg.session_token,
                backend_statuses: statuses,
            })
            .await
            .unwrap();

        assert!(resp.acknowledged);
    }
}
