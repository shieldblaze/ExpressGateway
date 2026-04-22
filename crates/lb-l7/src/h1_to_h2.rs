//! HTTP/1.1 to HTTP/2 bridge.
//!
//! Converts HTTP/1.1 requests to HTTP/2 by adding pseudo-headers and removing
//! connection-specific headers that are not valid in HTTP/2.

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// Headers that must be removed when bridging from HTTP/1.1 to HTTP/2.
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "transfer-encoding",
    "upgrade",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
];

/// Collect the header names listed in the `Connection` header value.
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

/// Bridge that converts HTTP/1.1 requests into HTTP/2 format.
pub struct H1ToH2Bridge;

impl Bridge for H1ToH2Bridge {
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error> {
        check_header_count(req.headers.len())?;

        let conn_named = connection_named_headers(&req.headers);

        let mut pseudo_headers: Vec<(String, String)> = Vec::new();
        let mut regular_headers: Vec<(String, String)> = Vec::new();

        // Extract authority from Host header.
        let authority = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("host"))
            .map(|(_, v)| v.clone());

        // Build pseudo-headers.
        let scheme = req.scheme.as_deref().unwrap_or("https");
        pseudo_headers.push((":method".to_owned(), req.method.clone()));
        pseudo_headers.push((":path".to_owned(), req.uri.clone()));
        pseudo_headers.push((":scheme".to_owned(), scheme.to_owned()));

        if let Some(auth) = authority {
            pseudo_headers.push((":authority".to_owned(), auth));
        }

        // Filter out hop-by-hop headers, host (replaced by :authority), and
        // any headers named in the Connection value.
        for (k, v) in &req.headers {
            let lower = k.to_lowercase();
            if lower == "host" {
                continue;
            }
            // Allow TE only if the value is "trailers" (RFC 7540 sect. 8.1.2.2).
            if lower == "te" {
                if v.trim().eq_ignore_ascii_case("trailers") {
                    regular_headers.push((lower, "trailers".to_owned()));
                }
                continue;
            }
            if HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower.as_str()) {
                continue;
            }
            if conn_named.iter().any(|n| *n == lower) {
                continue;
            }
            regular_headers.push((lower, v.clone()));
        }

        pseudo_headers.append(&mut regular_headers);
        check_header_count(pseudo_headers.len())?;

        Ok(BridgeRequest {
            method: req.method.clone(),
            uri: req.uri.clone(),
            headers: pseudo_headers,
            body: req.body.clone(),
            scheme: req.scheme.clone(),
        })
    }

    fn bridge_response(&self, resp: &BridgeResponse) -> Result<BridgeResponse, L7Error> {
        check_header_count(resp.headers.len())?;

        let conn_named = connection_named_headers(&resp.headers);

        let headers: Vec<(String, String)> = resp
            .headers
            .iter()
            .filter(|(k, _)| {
                let lower = k.to_lowercase();
                if HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower.as_str()) {
                    return false;
                }
                if conn_named.iter().any(|n| *n == lower) {
                    return false;
                }
                true
            })
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
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
        Protocol::Http2
    }
}
