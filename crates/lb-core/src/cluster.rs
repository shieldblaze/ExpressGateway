//! Cluster: a named group of backends with a load balancing policy.

use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::policy::LbPolicy;

/// A named cluster of backend servers using a specific load balancing policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    name: String,
    backends: Vec<Backend>,
    policy: LbPolicy,
}

impl Cluster {
    /// Create a new cluster.
    #[must_use]
    pub const fn new(name: String, backends: Vec<Backend>, policy: LbPolicy) -> Self {
        Self {
            name,
            backends,
            policy,
        }
    }

    /// Returns the cluster name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the backends in this cluster.
    #[must_use]
    pub fn backends(&self) -> &[Backend] {
        &self.backends
    }

    /// Returns a mutable reference to the backends in this cluster.
    pub fn backends_mut(&mut self) -> &mut Vec<Backend> {
        &mut self.backends
    }

    /// Returns the load balancing policy for this cluster.
    #[must_use]
    pub const fn policy(&self) -> &LbPolicy {
        &self.policy
    }
}
