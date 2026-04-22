//! HTTP/1.1 request line, status line, and header parsing.

use http::{Method, StatusCode, Uri, Version};

use crate::H1Error;

/// Default maximum header-section size for HTTP/1.x parsing.
///
/// Covers the request/response line, every header line, and the
/// terminating blank line. Chosen to match common production settings
/// (nginx `large_client_header_buffers`, Apache `LimitRequestFieldSize`).
pub const MAX_HEADER_BYTES: usize = 65_536;

/// Find the position of `\r\n` in `buf`, returning the index of `\r`.
fn find_crlf(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(1))
        .find(|&i| buf.get(i).copied() == Some(b'\r') && buf.get(i + 1).copied() == Some(b'\n'))
}

/// Find the position of `\r\n\r\n` in `buf`, returning the index of the first `\r`.
fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    let len = buf.len();
    (0..len.saturating_sub(3)).find(|&i| {
        buf.get(i).copied() == Some(b'\r')
            && buf.get(i + 1).copied() == Some(b'\n')
            && buf.get(i + 2).copied() == Some(b'\r')
            && buf.get(i + 3).copied() == Some(b'\n')
    })
}

/// Parse an HTTP/1.x request line from the beginning of `buf`.
///
/// Returns `(Method, Uri, Version, bytes_consumed)` on success.
/// `bytes_consumed` includes the trailing `\r\n`.
///
/// # Errors
///
/// Returns `H1Error::Incomplete` if the buffer does not contain a complete line.
/// Returns `H1Error::InvalidRequestLine` if the line is malformed.
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

/// Parse an HTTP/1.x status line from the beginning of `buf`.
///
/// Returns `(Version, StatusCode, bytes_consumed)` on success.
///
/// # Errors
///
/// Returns `H1Error::Incomplete` if the buffer does not contain a complete line.
/// Returns `H1Error::InvalidStatusLine` if the line is malformed.
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

/// Parse headers from `buf` until the blank line (`\r\n\r\n`), with the
/// default header-section size cap (`MAX_HEADER_BYTES`).
///
/// Returns the list of `(name, value)` pairs and total bytes consumed
/// (including the trailing `\r\n\r\n`).
///
/// # Errors
///
/// Returns `H1Error::Incomplete` if the terminating blank line is not found.
/// Returns `H1Error::InvalidHeader` if any header line is malformed.
/// Returns `H1Error::HeadersTooLarge` if the header section is longer than
/// `MAX_HEADER_BYTES` bytes (see `parse_headers_with_limit`).
pub fn parse_headers(buf: &[u8]) -> Result<(Vec<(String, String)>, usize), H1Error> {
    parse_headers_with_limit(buf, MAX_HEADER_BYTES)
}

/// Parse headers from `buf` until the blank line (`\r\n\r\n`), enforcing
/// the supplied `max_header_bytes` cap on the header-section length.
///
/// The cap is checked *before* any parsing work: if the buffer already
/// contains more than `max_header_bytes` bytes with no terminator in
/// sight, the function returns `HeadersTooLarge` rather than waiting for
/// more data that would only grow the section further. When the
/// terminator is found, the total consumed-byte count (up to and
/// including the trailing `\r\n\r\n`) is re-checked against the limit.
///
/// # Errors
///
/// * `H1Error::Incomplete` — terminator not yet observed *and* the
///   already-buffered bytes are still within the cap.
/// * `H1Error::HeadersTooLarge` — header section exceeds `max_header_bytes`.
/// * `H1Error::InvalidHeader` — a header line is malformed.
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

    // Include the trailing \r\n so the last header line has its CRLF terminator.
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

        let name = line_str
            .get(..colon_pos)
            .ok_or_else(|| H1Error::InvalidHeader(line_str.to_string()))?
            .trim()
            .to_string();
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

/// Parse trailer headers. Same format as regular headers, after the final chunk.
///
/// Uses the default `MAX_HEADER_BYTES` cap.
///
/// # Errors
///
/// Returns `H1Error::Incomplete` if the terminating blank line is not found.
/// Returns `H1Error::InvalidHeader` if any trailer line is malformed.
/// Returns `H1Error::HeadersTooLarge` if the trailer section exceeds the cap.
pub fn parse_trailers(buf: &[u8]) -> Result<(Vec<(String, String)>, usize), H1Error> {
    parse_headers_with_limit(buf, MAX_HEADER_BYTES)
}

/// Parse trailer headers with an explicit byte cap.
///
/// # Errors
///
/// See `parse_headers_with_limit`.
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
        // Build a header section whose final `\r\n\r\n` byte lands on
        // exactly the configured limit. Single header line
        // "X: <pad>\r\n" + final "\r\n" → 3 + pad + 2 + 2 bytes.
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
        // Header section is 70 bytes, limit is 64 — HeadersTooLarge.
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
        // No terminator and the buffer already exceeds the cap — fail fast
        // rather than wait for more data.
        let buf = vec![b'a'; 129];
        let err = parse_headers_with_limit(&buf, 128).unwrap_err();
        assert!(matches!(err, H1Error::HeadersTooLarge { .. }));
    }

    #[test]
    fn default_constant_matches_wrapper() {
        assert_eq!(MAX_HEADER_BYTES, 65_536);
        // The zero-arg wrapper uses MAX_HEADER_BYTES. A tiny well-formed
        // header must still succeed via the wrapper.
        let buf = b"Host: x\r\n\r\n";
        let (headers, consumed) = parse_headers(buf).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(consumed, buf.len());
    }
}
