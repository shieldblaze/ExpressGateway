//! gRPC service stubs for the control plane.
//!
//! These define the service trait signatures that will be implemented
//! when proto files are set up with tonic-build. For now, they serve
//! as placeholders documenting the intended gRPC API surface.

use serde::{Deserialize, Serialize};

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

// ---------------------------------------------------------------------------
// Service trait definitions (to be implemented with tonic later)
// ---------------------------------------------------------------------------

/// Node registration and heartbeat service.
///
/// TODO: Implement with tonic when proto files are available.
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
/// TODO: Implement with tonic when proto files are available.
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
/// TODO: Implement with tonic when proto files are available.
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
