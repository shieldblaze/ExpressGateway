//! HTTP/1.1 to HTTP/1.1 bridge (pass-through with hop-by-hop stripping).

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// HTTP/1.1 hop-by-hop headers that MUST NOT be forwarded by a proxy
/// (RFC 7230 section 6.1).
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
];

/// Collect the header names listed in the `Connection` header value.
///
/// `Connection: keep-alive, x-custom` -> `["keep-alive", "x-custom"]`.
fn connection_named_headers(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty())
        })
        .collect()
}

/// Returns `true` if `te_value` is exactly `"trailers"` (case-insensitive).
fn is_te_trailers(te_value: &str) -> bool {
    te_value.trim().eq_ignore_ascii_case("trailers")
}

/// Bridge that passes HTTP/1.1 requests through, stripping hop-by-hop headers.
pub struct H1ToH1Bridge;

impl Bridge for H1ToH1Bridge {
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error> {
        check_header_count(req.headers.len())?;

        let conn_named = connection_named_headers(&req.headers);

        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .filter_map(|(k, v)| {
                let lower = k.to_lowercase();
                // Allow TE only if the value is "trailers".
                if lower == "te" {
                    return if is_te_trailers(v) {
                        Some((lower, "trailers".to_owned()))
                    } else {
                        None
                    };
                }
                if HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower.as_str()) {
                    return None;
                }
                if conn_named.iter().any(|n| *n == lower) {
                    return None;
                }
                Some((lower, v.clone()))
            })
            .collect();

        check_header_count(headers.len())?;

        Ok(BridgeRequest {
            method: req.method.clone(),
            uri: req.uri.clone(),
            headers,
            body: req.body.clone(),
            scheme: req.scheme.clone(),
        })
    }

    fn bridge_response(&self, resp: &BridgeResponse) -> Result<BridgeResponse, L7Error> {
        check_header_count(resp.headers.len())?;

        let conn_named = connection_named_headers(&resp.headers);

        // TE is a request-only header (RFC 7230 section 4.3) so unlike the
        // request path we do NOT preserve "te: trailers" in responses — it is
        // unconditionally stripped via the hop-by-hop list.
        let headers: Vec<(String, String)> = resp
            .headers
            .iter()
            .filter_map(|(k, v)| {
                let lower = k.to_lowercase();
                if HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower.as_str()) {
                    return None;
                }
                if conn_named.iter().any(|n| *n == lower) {
                    return None;
                }
                Some((lower, v.clone()))
            })
            .collect();

        check_header_count(headers.len())?;

        Ok(BridgeResponse {
            status: resp.status,
            headers,
            body: resp.body.clone(),
        })
    }

    fn source_protocol(&self) -> Protocol {
        Protocol::Http1
    }

    fn dest_protocol(&self) -> Protocol {
        Protocol::Http1
    }
}
