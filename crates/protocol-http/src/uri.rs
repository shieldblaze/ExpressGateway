//! URI normalization per RFC 3986 Section 5.2.4.
//!
//! Provides path normalization (dot-segment removal), path-traversal detection,
//! null-byte rejection, and double-encoded dot detection.

/// Result of URI normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUri {
    /// The normalized path component.
    pub path: String,
    /// The original query string (preserved as-is).
    pub query: Option<String>,
}

/// Errors that can occur during URI normalization.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UriError {
    /// Path traversal detected (e.g., `/../` escaping the root).
    #[error("path traversal detected: {path}")]
    PathTraversal { path: String },

    /// Null byte found in the URI.
    #[error("null byte in URI")]
    NullByte,

    /// Double-encoded dot detected (e.g., `%252e`).
    #[error("double-encoded dot detected")]
    DoubleEncodedDot,
}

/// Normalize a URI path and query, rejecting malicious patterns.
///
/// This function:
/// 1. Rejects null bytes (`%00`)
/// 2. Rejects double-encoded dots (`%252e`, `%252E`)
/// 3. Removes dot segments per RFC 3986 Section 5.2.4
/// 4. Rejects paths that traverse above the root
/// 5. Preserves the query string unchanged
pub fn normalize(uri: &str) -> Result<NormalizedUri, UriError> {
    // Split path and query.
    let (path_part, query) = match uri.find('?') {
        Some(idx) => (&uri[..idx], Some(uri[idx + 1..].to_string())),
        None => (uri, None),
    };

    // Check for null bytes (literal or percent-encoded).
    if path_part.contains('\0') || contains_encoded_null(path_part) {
        return Err(UriError::NullByte);
    }

    // Check for double-encoded dots: %252e or %252E
    if contains_double_encoded_dot(path_part) {
        return Err(UriError::DoubleEncodedDot);
    }

    // Remove dot segments per RFC 3986 Section 5.2.4.
    let normalized = remove_dot_segments(path_part);

    // After normalization, check for path traversal (segments that would escape root).
    // If the original had `/../` patterns that resolved above root, the remove_dot_segments
    // algorithm handles it, but we also reject explicit traversal attempts.
    if has_traversal_attempt(path_part) {
        return Err(UriError::PathTraversal {
            path: path_part.to_string(),
        });
    }

    Ok(NormalizedUri {
        path: if normalized.is_empty() {
            "/".to_string()
        } else {
            normalized
        },
        query,
    })
}

/// Check if a path contains percent-encoded null bytes (`%00`).
fn contains_encoded_null(path: &str) -> bool {
    let bytes = path.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 2 < len {
        if bytes[i] == b'%' && bytes[i + 1] == b'0' && bytes[i + 2] == b'0' {
            return true;
        }
        i += 1;
    }
    false
}

/// Check if a path contains double-encoded dots (`%252e` or `%252E`).
fn contains_double_encoded_dot(path: &str) -> bool {
    // %252e decodes to %2e which decodes to `.`
    let lower = path.to_ascii_lowercase();
    lower.contains("%252e")
}

/// Check if a raw path has a traversal attempt that would escape the root.
///
/// We count depth: each normal segment increments, each `..` decrements.
/// If depth ever goes negative, it is a traversal.
fn has_traversal_attempt(path: &str) -> bool {
    let mut depth: i32 = 0;
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            depth -= 1;
            if depth < 0 {
                return true;
            }
        } else {
            depth += 1;
        }
    }
    false
}

/// Remove dot segments from a path per RFC 3986 Section 5.2.4.
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();

    for segment in path.split('/') {
        match segment {
            "." => {
                // Skip single dot segments.
            }
            ".." => {
                // Pop the last segment (go up one level).
                output.pop();
            }
            _ => {
                output.push(segment);
            }
        }
    }

    // Reconstruct path.
    let result = output.join("/");

    // Preserve leading slash.
    if path.starts_with('/') && !result.starts_with('/') {
        format!("/{result}")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_path_unchanged() {
        let result = normalize("/foo/bar").unwrap();
        assert_eq!(result.path, "/foo/bar");
        assert_eq!(result.query, None);
    }

    #[test]
    fn removes_single_dot() {
        let result = normalize("/foo/./bar").unwrap();
        assert_eq!(result.path, "/foo/bar");
    }

    #[test]
    fn removes_double_dot() {
        let result = normalize("/foo/baz/../bar").unwrap();
        assert_eq!(result.path, "/foo/bar");
    }

    #[test]
    fn preserves_query_string() {
        let result = normalize("/foo/bar?key=value&a=b").unwrap();
        assert_eq!(result.path, "/foo/bar");
        assert_eq!(result.query, Some("key=value&a=b".into()));
    }

    #[test]
    fn rejects_path_traversal() {
        let result = normalize("/../etc/passwd");
        assert!(matches!(result, Err(UriError::PathTraversal { .. })));
    }

    #[test]
    fn rejects_null_byte() {
        let result = normalize("/foo%00bar");
        assert!(matches!(result, Err(UriError::NullByte)));
    }

    #[test]
    fn rejects_literal_null_byte() {
        let result = normalize("/foo\0bar");
        assert!(matches!(result, Err(UriError::NullByte)));
    }

    #[test]
    fn rejects_double_encoded_dot() {
        let result = normalize("/foo/%252e%252e/bar");
        assert!(matches!(result, Err(UriError::DoubleEncodedDot)));
    }

    #[test]
    fn rejects_double_encoded_dot_uppercase() {
        let result = normalize("/foo/%252E%252E/bar");
        assert!(matches!(result, Err(UriError::DoubleEncodedDot)));
    }

    #[test]
    fn root_path() {
        let result = normalize("/").unwrap();
        assert_eq!(result.path, "/");
    }

    #[test]
    fn complex_dot_segments() {
        let result = normalize("/a/b/c/./../../g").unwrap();
        assert_eq!(result.path, "/a/g");
    }

    #[test]
    fn trailing_dot() {
        let result = normalize("/foo/bar/.").unwrap();
        assert_eq!(result.path, "/foo/bar");
    }

    #[test]
    fn empty_path_becomes_root() {
        let result = normalize("").unwrap();
        assert_eq!(result.path, "/");
    }

    #[test]
    fn safe_double_dot_does_not_escape_root() {
        // /a/../b is safe -- depth goes 1 then 0 then 1.
        let result = normalize("/a/../b").unwrap();
        assert_eq!(result.path, "/b");
    }

    #[test]
    fn multiple_traversal_attempt() {
        let result = normalize("/a/../../etc/passwd");
        assert!(matches!(result, Err(UriError::PathTraversal { .. })));
    }
}
