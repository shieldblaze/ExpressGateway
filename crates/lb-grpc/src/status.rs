//! gRPC status codes and HTTP status translation.
//!
//! Maps between gRPC status codes (0..=16) and HTTP status codes per the
//! gRPC specification:
//! <https://github.com/grpc/grpc/blob/master/doc/statuscodes.md>

use crate::GrpcError;

/// gRPC status codes as defined in the gRPC specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcStatus {
    /// Not an error; returned on success.
    Ok = 0,
    /// The operation was cancelled.
    Cancelled = 1,
    /// Unknown error.
    Unknown = 2,
    /// Client specified an invalid argument.
    InvalidArgument = 3,
    /// Deadline expired before operation could complete.
    DeadlineExceeded = 4,
    /// Some requested entity was not found.
    NotFound = 5,
    /// Entity that a client attempted to create already exists.
    AlreadyExists = 6,
    /// The caller does not have permission.
    PermissionDenied = 7,
    /// Some resource has been exhausted.
    ResourceExhausted = 8,
    /// Operation was rejected because the system is not in a required state.
    FailedPrecondition = 9,
    /// The operation was aborted.
    Aborted = 10,
    /// Operation was attempted past the valid range.
    OutOfRange = 11,
    /// Operation is not implemented.
    Unimplemented = 12,
    /// Internal errors.
    Internal = 13,
    /// The service is currently unavailable.
    Unavailable = 14,
    /// Unrecoverable data loss or corruption.
    DataLoss = 15,
    /// The request does not have valid authentication credentials.
    Unauthenticated = 16,
}

impl GrpcStatus {
    /// Parse a gRPC status code from its numeric representation.
    ///
    /// # Errors
    ///
    /// Returns [`GrpcError::InvalidStatus`] if the code is outside 0..=16.
    pub const fn from_code(code: u32) -> Result<Self, GrpcError> {
        match code {
            0 => Ok(Self::Ok),
            1 => Ok(Self::Cancelled),
            2 => Ok(Self::Unknown),
            3 => Ok(Self::InvalidArgument),
            4 => Ok(Self::DeadlineExceeded),
            5 => Ok(Self::NotFound),
            6 => Ok(Self::AlreadyExists),
            7 => Ok(Self::PermissionDenied),
            8 => Ok(Self::ResourceExhausted),
            9 => Ok(Self::FailedPrecondition),
            10 => Ok(Self::Aborted),
            11 => Ok(Self::OutOfRange),
            12 => Ok(Self::Unimplemented),
            13 => Ok(Self::Internal),
            14 => Ok(Self::Unavailable),
            15 => Ok(Self::DataLoss),
            16 => Ok(Self::Unauthenticated),
            _ => Err(GrpcError::InvalidStatus(code)),
        }
    }

    /// Map this gRPC status to the closest HTTP status code.
    ///
    /// Per the gRPC spec's mapping table for gRPC-to-HTTP translation.
    #[must_use]
    pub const fn to_http_status(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::Cancelled => 499, // Client Closed Request (nginx convention)
            Self::Unknown | Self::Internal | Self::DataLoss => 500,
            Self::InvalidArgument | Self::FailedPrecondition | Self::OutOfRange => 400,
            Self::DeadlineExceeded => 504,
            Self::NotFound => 404,
            Self::AlreadyExists | Self::Aborted => 409,
            Self::PermissionDenied => 403,
            Self::ResourceExhausted => 429,
            Self::Unimplemented => 501,
            Self::Unavailable => 503,
            Self::Unauthenticated => 401,
        }
    }

    /// Map an HTTP status code to the closest gRPC status code.
    ///
    /// Per the gRPC spec's mapping table for HTTP-to-gRPC translation.
    #[must_use]
    pub const fn from_http_status(status: u16) -> Self {
        match status {
            200 => Self::Ok,
            400 | 500 => Self::Internal,
            401 => Self::Unauthenticated,
            403 => Self::PermissionDenied,
            404 | 501 => Self::Unimplemented,
            409 => Self::Aborted,
            429 | 502..=504 => Self::Unavailable,
            499 => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}
