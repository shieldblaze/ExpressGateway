//! Error types for the lb-core crate.

/// Errors that can occur within core load balancer types.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A backend was specified with an invalid weight.
    #[error("backend '{backend_id}' has invalid weight: {weight}")]
    InvalidWeight {
        /// The backend identifier.
        backend_id: String,
        /// The invalid weight value.
        weight: u32,
    },

    /// The cluster has no backends.
    #[error("cluster '{cluster_name}' has no backends")]
    EmptyCluster {
        /// The cluster name.
        cluster_name: String,
    },

    /// A backend with the given id was not found.
    #[error("backend '{backend_id}' not found")]
    BackendNotFound {
        /// The backend identifier.
        backend_id: String,
    },
}
