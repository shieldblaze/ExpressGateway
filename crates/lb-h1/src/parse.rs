//! HTTP/1.1 request line, status line, and header parsing.

use http::{Method, StatusCode, Uri, Version};

use crate::H1Error;

/// Default header-section cap: the start line, every header line, and the
/// terminating blank line (matching nginx / Apache production settings).
pub const MAX_HEADER_BYTES: usize = 65_536;

/// Re-export for the chunked trailer parser (the ROUND8-L7-03 mirror).
#[doc(hidden)]
pub(crate) const fn __is_tchar_for_trailer(b: u8) -> bool {
    is_tchar(b)
}

/// RFC 9110 §5.6.2 `tchar` predicate; ROUND8-L7-03 rejects everything else.
const fn is_tchar(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
        | b'$'
        | b'%'
        | b'&'
        | b'\''
        | b'*'
        | b'+'
        | b'-'
        | b'.'
        | b'^'
        | b'_'
        | b'`'
        | b'|'
        | b'~'
        | b'0'..=b'9'
        | b'a'..=b'z'
        | b'A'..=b'Z'
    )
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(1))
        .find(|&i| buf.get(i).copied() == Some(b'\r') && buf.get(i + 1).copied() == Some(b'\n'))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(3)).find(|&i| {
        buf.get(i).copied() == Some(b'\r')
            && buf.get(i + 1).copied() == Some(b'\n')
            && buf.get(i + 2).copied() == Some(b'\r')
            && buf.get(i + 3).copied() == Some(b'\n')
    })
}

/// Parse a request line; `bytes_consumed` includes the trailing CRLF.
///
/// # Errors
/// `Incomplete` on a partial line, `InvalidRequestLine` if malformed.
pub fn parse_request_line(buf: &[u8]) -> Result<(Method, Uri, Version, usize), H1Error> {
    let crlf_pos = find_crlf(buf).ok_or(H1Error::Incomplete)?;
    let line = buf.get(..crlf_pos).ok_or(H1Error::InvalidRequestLine)?;
    let line_str = core::str::from_utf8(line).map_err(|_| H1Error::InvalidRequestLine)?;

    let mut parts = line_str.splitn(3, ' ');
    let method_str = parts.next().ok_or(H1Error::InvalidRequestLine)?;
    let uri_str = parts.next().ok_or(H1Error::InvalidRequestLine)?;
    let version_str = parts.next().ok_or(H1Error::InvalidRequestLine)?;

    let method: Method = method_str
        .parse()
        .map_err(|_| H1Error::InvalidRequestLine)?;
    let uri: Uri = uri_str.parse().map_err(|_| H1Error::InvalidRequestLine)?;
    let version = parse_version(version_str)?;

    Ok((method, uri, version, crlf_pos + 2))
}

/// Parse a status line into `(Version, StatusCode, bytes_consumed)`.
///
/// # Errors
/// `Incomplete` on a partial line, `InvalidStatusLine` if malformed.
pub fn parse_status_line(buf: &[u8]) -> Result<(Version, StatusCode, usize), H1Error> {
    let crlf_pos = find_crlf(buf).ok_or(H1Error::Incomplete)?;
    let line = buf.get(..crlf_pos).ok_or(H1Error::InvalidStatusLine)?;
    let line_str = core::str::from_utf8(line).map_err(|_| H1Error::InvalidStatusLine)?;

    let mut parts = line_str.splitn(3, ' ');
    let version_str = parts.next().ok_or(H1Error::InvalidStatusLine)?;
    let code_str = parts.next().ok_or(H1Error::InvalidStatusLine)?;

    let version = parse_version(version_str).map_err(|_| H1Error::InvalidStatusLine)?;
    let code: u16 = code_str.parse().map_err(|_| H1Error::InvalidStatusLine)?;
    let status = StatusCode::from_u16(code).map_err(|_| H1Error::InvalidStatusLine)?;

    Ok((version, status, crlf_pos + 2))
}

/// Parse headers up to the blank line under the default `MAX_HEADER_BYTES`.
///
/// # Errors
/// See [`parse_headers_with_limit`].
pub fn parse_headers(buf: &[u8]) -> Result<(Vec<(String, String)>, usize), H1Error> {
    parse_headers_with_limit(buf, MAX_HEADER_BYTES)
}

/// Parse headers up to the blank line under an explicit `max_header_bytes`.
///
/// The cap is checked BEFORE any parsing work, so an over-cap buffer with no
/// terminator in sight fails fast rather than waiting for more data that would
/// only grow the section.
///
/// # Errors
/// `Incomplete` (terminator unseen and still within the cap),
/// `HeadersTooLarge`, or `InvalidHeader`.
pub fn parse_headers_with_limit(
    buf: &[u8],
    max_header_bytes: usize,
) -> Result<(Vec<(String, String)>, usize), H1Error> {
    let Some(end) = find_double_crlf(buf) else {
        if buf.len() > max_header_bytes {
            return Err(H1Error::HeadersTooLarge {
                limit: max_header_bytes,
                observed: buf.len(),
            });
        }
        return Err(H1Error::Incomplete);
    };

    let total_consumed = end + 4;
    if total_consumed > max_header_bytes {
        return Err(H1Error::HeadersTooLarge {
            limit: max_header_bytes,
            observed: total_consumed,
        });
    }

    // Include the trailing CRLF so the last header line is terminated.
    let header_block = buf.get(..end + 2).ok_or(H1Error::Incomplete)?;

    let mut headers = Vec::new();
    let mut pos = 0;

    while pos < header_block.len() {
        let remaining = header_block.get(pos..).ok_or(H1Error::Incomplete)?;
        let line_end = find_crlf(remaining)
            .ok_or_else(|| H1Error::InvalidHeader("missing CRLF".to_string()))?;

        let line = remaining.get(..line_end).ok_or(H1Error::Incomplete)?;
        let line_str = core::str::from_utf8(line)
            .map_err(|_| H1Error::InvalidHeader("non-utf8 header".to_string()))?;

        let colon_pos = line_str
            .find(':')
            .ok_or_else(|| H1Error::InvalidHeader(line_str.to_string()))?;

        // ROUND8-L7-03 / HAProxy CVE-2023-25725 / nginx CVE-2019-9516: the raw
        // bytes BEFORE the colon must be non-empty and all tchar. Deliberately
        // NOT trimmed — whitespace there is itself a violation (RFC 9112 §5.1).
        let raw_name = line_str
            .get(..colon_pos)
            .ok_or_else(|| H1Error::InvalidHeader(line_str.to_string()))?;
        if raw_name.is_empty() {
            return Err(H1Error::InvalidHeader("empty header name".to_string()));
        }
        if !raw_name.bytes().all(is_tchar) {
            return Err(H1Error::InvalidHeader(
                "non-tchar in header name".to_string(),
            ));
        }
        let name = raw_name.to_string();
        let value = line_str
            .get(colon_pos + 1..)
            .ok_or_else(|| H1Error::InvalidHeader(line_str.to_string()))?
            .trim()
            .to_string();

        headers.push((name, value));
        pos += line_end + 2;
    }

    Ok((headers, total_consumed))
}

/// Parse trailer headers under the default `MAX_HEADER_BYTES` cap.
///
/// # Errors
/// See [`parse_headers_with_limit`].
pub fn parse_trailers(buf: &[u8]) -> Result<(Vec<(String, String)>, usize), H1Error> {
    parse_headers_with_limit(buf, MAX_HEADER_BYTES)
}

/// Parse trailer headers with an explicit byte cap.
///
/// # Errors
/// See [`parse_headers_with_limit`].
pub fn parse_trailers_with_limit(
    buf: &[u8],
    max_header_bytes: usize,
) -> Result<(Vec<(String, String)>, usize), H1Error> {
    parse_headers_with_limit(buf, max_header_bytes)
}

/// Parse an HTTP version string like `HTTP/1.1` or `HTTP/1.0`.
fn parse_version(s: &str) -> Result<Version, H1Error> {
    match s {
        "HTTP/1.0" => Ok(Version::HTTP_10),
        "HTTP/1.1" => Ok(Version::HTTP_11),
        _ => Err(H1Error::InvalidRequestLine),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_line_get() {
        let buf = b"GET /index.html HTTP/1.1\r\n";
        let (method, uri, version, consumed) = parse_request_line(buf).unwrap();
        assert_eq!(method, Method::GET);
        assert_eq!(uri, "/index.html");
        assert_eq!(version, Version::HTTP_11);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn request_line_post_http10() {
        let buf = b"POST /api HTTP/1.0\r\n";
        let (method, _uri, version, _) = parse_request_line(buf).unwrap();
        assert_eq!(method, Method::POST);
        assert_eq!(version, Version::HTTP_10);
    }

    #[test]
    fn request_line_incomplete() {
        let buf = b"GET /path HTTP/1.1";
        assert!(matches!(parse_request_line(buf), Err(H1Error::Incomplete)));
    }

    #[test]
    fn status_line_200() {
        let buf = b"HTTP/1.1 200 OK\r\n";
        let (version, status, consumed) = parse_status_line(buf).unwrap();
        assert_eq!(version, Version::HTTP_11);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn status_line_404() {
        let buf = b"HTTP/1.0 404 Not Found\r\n";
        let (version, status, _) = parse_status_line(buf).unwrap();
        assert_eq!(version, Version::HTTP_10);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn headers_basic() {
        let buf = b"Content-Type: text/html\r\nContent-Length: 42\r\n\r\n";
        let (headers, consumed) = parse_headers(buf).unwrap();
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers[0],
            ("Content-Type".to_string(), "text/html".to_string())
        );
        assert_eq!(headers[1], ("Content-Length".to_string(), "42".to_string()));
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn headers_incomplete() {
        let buf = b"Content-Type: text/html\r\n";
        assert!(matches!(parse_headers(buf), Err(H1Error::Incomplete)));
    }

    #[test]
    fn header_exactly_at_limit_accepted() {
        // The final CRLFCRLF byte must land on EXACTLY the limit.
        let prefix = b"X: ";
        let limit = 64usize;
        let pad = limit - prefix.len() - 2 - 2;
        let mut buf = Vec::new();
        buf.extend_from_slice(prefix);
        buf.extend(std::iter::repeat_n(b'a', pad));
        buf.extend_from_slice(b"\r\n\r\n");
        assert_eq!(buf.len(), limit);

        let (headers, consumed) = parse_headers_with_limit(&buf, limit).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].0, "X");
        assert_eq!(headers[0].1, "a".repeat(pad));
        assert_eq!(consumed, limit);
    }

    #[test]
    fn header_over_limit_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"X: ");
        buf.extend(std::iter::repeat_n(b'a', 70 - 3 - 4));
        buf.extend_from_slice(b"\r\n\r\n");
        assert_eq!(buf.len(), 70);

        let err = parse_headers_with_limit(&buf, 64).unwrap_err();
        assert!(
            matches!(
                err,
                H1Error::HeadersTooLarge {
                    limit: 64,
                    observed: 70
                }
            ),
            "expected HeadersTooLarge {{limit:64, observed:70}}, got {err:?}",
        );
    }

    #[test]
    fn header_unterminated_over_limit_rejected() {
        // No terminator and already over the cap — must fail fast.
        let buf = vec![b'a'; 129];
        let err = parse_headers_with_limit(&buf, 128).unwrap_err();
        assert!(matches!(err, H1Error::HeadersTooLarge { .. }));
    }

    #[test]
    fn default_constant_matches_wrapper() {
        assert_eq!(MAX_HEADER_BYTES, 65_536);
        let buf = b"Host: x\r\n\r\n";
        let (headers, consumed) = parse_headers(buf).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(consumed, buf.len());
    }
}
