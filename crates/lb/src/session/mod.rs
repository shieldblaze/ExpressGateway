//! Session persistence implementations.
//!
//! All types implement [`SessionPersistence`] from `expressgateway-core` for sticky
//! routing decisions.

pub mod consistent_hash;
pub mod four_tuple;
pub mod source_ip;

pub use consistent_hash::ConsistentHashPersistence;
pub use four_tuple::FourTuplePersistence;
pub use source_ip::SourceIpPersistence;
