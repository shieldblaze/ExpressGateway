//! HTTP/3 to HTTP/1.1 bridge.
//!
//! Identical transformation to [`crate::h2_to_h1`] since HTTP/3 uses the same
//! pseudo-header scheme as HTTP/2.

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// Hop-by-hop headers that must not appear in a forwarded HTTP/1.1 response
/// when the upstream was HTTP/3.
const RESPONSE_HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
];

/// Bridge that converts HTTP/3 requests into HTTP/1.1 format.
pub struct H3ToH1Bridge;

impl Bridge for H3ToH1Bridge {
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error> {
        check_header_count(req.headers.len())?;

        let mut method = req.method.clone();
        let mut uri = req.uri.clone();
        let mut authority: Option<String> = None;
        let mut regular_headers: Vec<(String, String)> = Vec::new();

        // Extract values from pseudo-headers, collect regular headers.
        for (k, v) in &req.headers {
            match k.as_str() {
                ":method" => method.clone_from(v),
                ":path" => uri.clone_from(v),
                ":scheme" => { /* Dropped in HTTP/1.1 -- scheme is implicit. */ }
                ":authority" => authority = Some(v.clone()),
                _ if k.starts_with(':') => {
                    // Skip any unknown pseudo-headers.
                }
                _ => {
                    regular_headers.push((k.to_lowercase(), v.clone()));
                }
            }
        }

        // :authority is required for a well-formed H3 request.
        // An empty value is treated the same as missing — it would produce an
        // invalid empty Host header in the downstream HTTP/1.1 request.
        let auth = authority
            .filter(|a| !a.is_empty())
            .ok_or_else(|| L7Error::MissingPseudoHeader(":authority".to_owned()))?;
        regular_headers.insert(0, ("host".to_owned(), auth));

        check_header_count(regular_headers.len())?;

        Ok(BridgeRequest {
            method,
            uri,
            headers: regular_headers,
            body: req.body.clone(),
            scheme: req.scheme.clone(),
        })
    }

    fn bridge_response(&self, resp: &BridgeResponse) -> Result<BridgeResponse, L7Error> {
        check_header_count(resp.headers.len())?;

        let headers: Vec<(String, String)> = resp
            .headers
            .iter()
            .filter(|(k, _)| {
                if k.starts_with(':') {
                    return false;
                }
                let lower = k.to_lowercase();
                !RESPONSE_HOP_BY_HOP.iter().any(|h| *h == lower.as_str())
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
        Protocol::Http3
    }

    fn dest_protocol(&self) -> Protocol {
        Protocol::Http1
    }
}
