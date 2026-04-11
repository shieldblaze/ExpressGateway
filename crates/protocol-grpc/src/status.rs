//! HTTP status code to gRPC status code mapping per the gRPC specification.
//!
//! All 17 gRPC status codes are defined per the gRPC spec. The `http_to_grpc`
//! mapping follows the canonical table from the gRPC over HTTP/2 spec. The
//! `from_code` constructor allows parsing status codes from wire trailers.

/// A gRPC status code (all 17 per the gRPC spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum GrpcStatus {
    Ok = 0,
    Cancelled = 1,
    Unknown = 2,
    InvalidArgument = 3,
    DeadlineExceeded = 4,
    NotFound = 5,
    AlreadyExists = 6,
    PermissionDenied = 7,
    ResourceExhausted = 8,
    FailedPrecondition = 9,
    Aborted = 10,
    OutOfRange = 11,
    Unimplemented = 12,
    Internal = 13,
    Unavailable = 14,
    DataLoss = 15,
    Unauthenticated = 16,
}

impl GrpcStatus {
    /// Returns the numeric gRPC status code.
    #[inline]
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Parse a numeric status code from the wire.
    ///
    /// Returns `None` for codes outside the defined range (0..=16).
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::Ok),
            1 => Some(Self::Cancelled),
            2 => Some(Self::Unknown),
            3 => Some(Self::InvalidArgument),
            4 => Some(Self::DeadlineExceeded),
            5 => Some(Self::NotFound),
            6 => Some(Self::AlreadyExists),
            7 => Some(Self::PermissionDenied),
            8 => Some(Self::ResourceExhausted),
            9 => Some(Self::FailedPrecondition),
            10 => Some(Self::Aborted),
            11 => Some(Self::OutOfRange),
            12 => Some(Self::Unimplemented),
            13 => Some(Self::Internal),
            14 => Some(Self::Unavailable),
            15 => Some(Self::DataLoss),
            16 => Some(Self::Unauthenticated),
            _ => None,
        }
    }

    /// Returns the canonical name of this status.
    pub fn name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Cancelled => "CANCELLED",
            Self::Unknown => "UNKNOWN",
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::NotFound => "NOT_FOUND",
            Self::AlreadyExists => "ALREADY_EXISTS",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::ResourceExhausted => "RESOURCE_EXHAUSTED",
            Self::FailedPrecondition => "FAILED_PRECONDITION",
            Self::Aborted => "ABORTED",
            Self::OutOfRange => "OUT_OF_RANGE",
            Self::Unimplemented => "UNIMPLEMENTED",
            Self::Internal => "INTERNAL",
            Self::Unavailable => "UNAVAILABLE",
            Self::DataLoss => "DATA_LOSS",
            Self::Unauthenticated => "UNAUTHENTICATED",
        }
    }

    /// Whether this status indicates success.
    #[inline]
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Map an HTTP/2 RST_STREAM error code to a gRPC status.
    ///
    /// Per the gRPC spec:
    /// - REFUSED_STREAM (0x7) -> UNAVAILABLE
    /// - CANCEL (0x8) -> CANCELLED
    /// - ENHANCE_YOUR_CALM (0xb) -> RESOURCE_EXHAUSTED
    /// - INADEQUATE_SECURITY (0xc) -> PERMISSION_DENIED
    /// - Everything else -> INTERNAL
    pub fn from_rst_stream(error_code: u32) -> Self {
        match error_code {
            0x0 => Self::Internal,        // NO_ERROR but unexpected RST
            0x7 => Self::Unavailable,     // REFUSED_STREAM
            0x8 => Self::Cancelled,       // CANCEL
            0xb => Self::ResourceExhausted, // ENHANCE_YOUR_CALM
            0xc => Self::PermissionDenied, // INADEQUATE_SECURITY
            _ => Self::Internal,
        }
    }
}

impl std::fmt::Display for GrpcStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name(), self.code())
    }
}

/// Maps an HTTP status code to the corresponding gRPC status code per the
/// gRPC over HTTP/2 specification.
///
/// This is used when the HTTP response has a non-200 status code, meaning the
/// gRPC call could not be completed at the transport level. For HTTP 200
/// responses, the gRPC status must be read from the `grpc-status` trailer --
/// HTTP 200 does not imply gRPC OK.
///
/// Returns `None` for HTTP status codes that have no defined gRPC mapping.
pub fn http_to_grpc(http_status: u16) -> Option<GrpcStatus> {
    match http_status {
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

    /// All 13 spec-mandated HTTP->gRPC mappings (200 excluded; see doc on
    /// `http_to_grpc`).
    #[test]
    fn all_13_mappings() {
        let cases: &[(u16, u32, &str)] = &[
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

    /// HTTP 200 must NOT map to gRPC OK. For gRPC, HTTP 200 is the expected
    /// transport status for all calls; the actual gRPC status comes from the
    /// `grpc-status` trailer.
    #[test]
    fn http_200_does_not_map_to_grpc_ok() {
        assert!(http_to_grpc(200).is_none());
    }

    #[test]
    fn display_impl() {
        assert_eq!(GrpcStatus::Ok.to_string(), "OK (0)");
        assert_eq!(GrpcStatus::Internal.to_string(), "INTERNAL (13)");
    }

    #[test]
    fn from_code_roundtrip() {
        for code in 0..=16u32 {
            let status = GrpcStatus::from_code(code).expect("valid code");
            assert_eq!(status.code(), code);
        }
        assert!(GrpcStatus::from_code(17).is_none());
        assert!(GrpcStatus::from_code(u32::MAX).is_none());
    }

    #[test]
    fn all_17_codes_have_names() {
        for code in 0..=16u32 {
            let status = GrpcStatus::from_code(code).unwrap();
            assert!(!status.name().is_empty());
        }
    }

    #[test]
    fn is_ok() {
        assert!(GrpcStatus::Ok.is_ok());
        assert!(!GrpcStatus::Internal.is_ok());
    }

    #[test]
    fn rst_stream_mapping() {
        assert_eq!(GrpcStatus::from_rst_stream(0x7), GrpcStatus::Unavailable);
        assert_eq!(GrpcStatus::from_rst_stream(0x8), GrpcStatus::Cancelled);
        assert_eq!(
            GrpcStatus::from_rst_stream(0xb),
            GrpcStatus::ResourceExhausted
        );
        assert_eq!(
            GrpcStatus::from_rst_stream(0xc),
            GrpcStatus::PermissionDenied
        );
        assert_eq!(GrpcStatus::from_rst_stream(0x1), GrpcStatus::Internal);
        assert_eq!(GrpcStatus::from_rst_stream(0x0), GrpcStatus::Internal);
    }
}
