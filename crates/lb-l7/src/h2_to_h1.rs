//! HTTP/2 to HTTP/1.1 bridge.
//!
//! Converts HTTP/2 pseudo-headers back to HTTP/1.1 request-line components
//! and a Host header.
//!
//! ## SEC-2-01 — H2→H1 downgrade smuggle check
//!
//! Wave-2b SEC-2-01 wires
//! [`lb_security::SmuggleDetector::check_h2_downgrade`] **before** the
//! H1 request-line is materialised. The downgrade path is the
//! highest-risk smuggle vector because H2 forbids hop-by-hop headers
//! (Connection, Keep-Alive, Transfer-Encoding, Upgrade) on the wire
//! but a malformed H2 frame can still carry them in the header block;
//! once we synthesise the H1 line, the upstream H1 parser sees the
//! forbidden header and a desynced response queue is one hop away.
//! On detection the bridge returns
//! [`L7Error::BridgeError`] carrying the smuggle reason; the proxy
//! call site renders that as a 400.

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// Hop-by-hop headers that must not appear in a forwarded HTTP/1.1 response
/// when the upstream was HTTP/2.
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

/// Bridge that converts HTTP/2 requests into HTTP/1.1 format.
pub struct H2ToH1Bridge;

impl Bridge for H2ToH1Bridge {
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

        // SEC-2-01 — H2→H1 downgrade smuggle check.
        //
        // Run the detector against the **regular** (non-pseudo)
        // header block once the pseudo-headers have been extracted.
        // Running on the raw inbound list would over-fire because
        // [`check_h2_downgrade`] treats any `:`-prefixed name as a
        // smuggle attempt — pseudo-headers are how H2 carries method
        // / path / scheme / authority, so they appear legitimately
        // here. The `is_h2_origin = true` flag enables the
        // [`check_h2_downgrade`] arm which rejects forbidden
        // hop-by-hop headers (Connection, Keep-Alive, Upgrade,
        // Transfer-Encoding, Proxy-Connection) and non-`trailers` TE
        // values per RFC 9113 §8.2.2. The CL/TE/duplicate-CL checks
        // also run defensively. This fires **before** the H1 request
        // line is materialised below.
        lb_security::SmuggleDetector::check_all(&regular_headers, /* is_h2_origin = */ true)
            .map_err(|e| L7Error::BridgeError(format!("h2->h1 downgrade smuggle: {e}")))?;

        // :authority is required for a well-formed H2 request.
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
        Protocol::Http2
    }

    fn dest_protocol(&self) -> Protocol {
        Protocol::Http1
    }
}
