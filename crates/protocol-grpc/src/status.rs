//! HTTP status code to gRPC status code mapping per the gRPC specification.

/// A gRPC status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum GrpcStatus {
    Ok = 0,
    Cancelled = 1,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    Aborted = 10,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    Unauthenticated = 16,
}

impl GrpcStatus {
    /// Returns the numeric gRPC status code.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Returns the canonical name of this status.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Cancelled => "CANCELLED",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::ResourceExhausted => "RESOURCE_EXHAUSTED",
            Self::Aborted => "ABORTED",
            Self::Unimplemented => "UNIMPLEMENTED",
            Self::Internal => "INTERNAL",
            Self::Unavailable => "UNAVAILABLE",
            Self::Unauthenticated => "UNAUTHENTICATED",
        }
    }
}

impl std::fmt::Display for GrpcStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name(), self.code())
    }
}

/// Maps an HTTP status code to the corresponding gRPC status code per the
/// gRPC specification.
///
/// Returns `None` for HTTP status codes that have no defined gRPC mapping.
pub fn http_to_grpc(http_status: u16) -> Option<GrpcStatus> {
    match http_status {
        200 => Some(GrpcStatus::Ok),
        400 => Some(GrpcStatus::InvalidArgument),
        401 => Some(GrpcStatus::Unauthenticated),
        403 => Some(GrpcStatus::PermissionDenied),
        404 => Some(GrpcStatus::Unimplemented),
        408 => Some(GrpcStatus::DeadlineExceeded),
        409 => Some(GrpcStatus::Aborted),
        429 => Some(GrpcStatus::ResourceExhausted),
        499 => Some(GrpcStatus::Cancelled),
        500 => Some(GrpcStatus::Internal),
        501 => Some(GrpcStatus::Unimplemented),
        502 => Some(GrpcStatus::Unavailable),
        503 => Some(GrpcStatus::Unavailable),
        504 => Some(GrpcStatus::DeadlineExceeded),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All 14 spec-mandated mappings.
    #[test]
    fn all_14_mappings() {
        let cases: &[(u16, u32, &str)] = &[
            (200, 0, "OK"),
            (400, 3, "INVALID_ARGUMENT"),
            (401, 16, "UNAUTHENTICATED"),
            (403, 7, "PERMISSION_DENIED"),
            (404, 12, "UNIMPLEMENTED"),
            (408, 4, "DEADLINE_EXCEEDED"),
            (409, 10, "ABORTED"),
            (429, 8, "RESOURCE_EXHAUSTED"),
            (499, 1, "CANCELLED"),
            (500, 13, "INTERNAL"),
            (501, 12, "UNIMPLEMENTED"),
            (502, 14, "UNAVAILABLE"),
            (503, 14, "UNAVAILABLE"),
            (504, 4, "DEADLINE_EXCEEDED"),
        ];

        for &(http, expected_code, expected_name) in cases {
            let grpc =
                http_to_grpc(http).unwrap_or_else(|| panic!("expected mapping for HTTP {http}"));
            assert_eq!(
                grpc.code(),
                expected_code,
                "HTTP {http} -> expected gRPC {expected_code}, got {}",
                grpc.code()
            );
            assert_eq!(
                grpc.name(),
                expected_name,
                "HTTP {http} -> expected name {expected_name}, got {}",
                grpc.name()
            );
        }
    }

    #[test]
    fn unmapped_status_returns_none() {
        assert!(http_to_grpc(201).is_none());
        assert!(http_to_grpc(301).is_none());
        assert!(http_to_grpc(418).is_none());
    }

    #[test]
    fn display_impl() {
        assert_eq!(GrpcStatus::Ok.to_string(), "OK (0)");
        assert_eq!(GrpcStatus::Internal.to_string(), "INTERNAL (13)");
    }
}
