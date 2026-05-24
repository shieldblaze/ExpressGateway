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
///
/// S11 I3 (D3): `pub(crate)` so the STREAMING H1←H2 response relay
/// (`h1_proxy::upstream_response_to_h1`) strips the SAME authoritative set as
/// the buffering `H2ToH1Bridge::bridge_response` path — single source of
/// truth, no copied list (mechanical, no behaviour change).
pub(crate) const RESPONSE_HOP_BY_HOP: &[&str] = &[
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

        // PROTO-2-01 — RFC 9113 §8.3.1: if the request carries both a
        // `:authority` pseudo-header and a regular `Host` header, the
        // two MUST agree. Mismatch is a host-confusion smuggling
        // primitive against backends that authorise on `Host` (the
        // bridge would otherwise replace `Host` with `:authority`
        // silently). The H2→H1 bridge is a separate entry point from
        // `H2Proxy::handle`; this guard catches direct bridge users
        // (test harnesses, future filter chains) as well as any
        // future code path that re-uses the bridge without the
        // proxy preamble.
        if let Some((idx, (_, existing_host))) = regular_headers
            .iter()
            .enumerate()
            .find(|(_, (k, _))| k.eq_ignore_ascii_case("host"))
        {
            if !authority_host_components_agree(&auth, existing_host) {
                return Err(L7Error::BridgeError(
                    "h2->h1 :authority/Host disagree (RFC 9113 §8.3.1)".to_owned(),
                ));
            }
            // Drop the existing Host so the inserted one below is the
            // sole entry. Keeps subsequent code unchanged.
            regular_headers.remove(idx);
        }

        regular_headers.insert(0, ("host".to_owned(), auth));

        check_header_count(regular_headers.len())?;

        Ok(BridgeRequest {
            method,
            uri,
            headers: regular_headers,
            body: req.body.clone(),
            scheme: req.scheme.clone(),
            // PROTO-2-12: forward request trailers.
            trailers: req.trailers.clone(),
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
            // PROTO-2-12: forward response trailers.
            trailers: resp.trailers.clone(),
        })
    }

    fn source_protocol(&self) -> Protocol {
        Protocol::Http2
    }

    fn dest_protocol(&self) -> Protocol {
        Protocol::Http1
    }
}

/// PROTO-2-01 — compare a `:authority` value against a `Host` header
/// value per RFC 9113 §8.3.1 + RFC 3986 §3.2.2.
///
/// Host case-insensitive; ports compared when both are explicit.
/// Either side eliding a port is accepted (default-port latitude).
/// Empty host on either side rejects.
fn authority_host_components_agree(authority: &str, host: &str) -> bool {
    let (a_host, a_port) = split_host_port(authority);
    let (h_host, h_port) = split_host_port(host);
    if a_host.is_empty() || h_host.is_empty() {
        return false;
    }
    if !a_host.eq_ignore_ascii_case(h_host) {
        return false;
    }
    match (a_port, h_port) {
        (Some(ap), Some(hp)) => ap == hp,
        _ => true,
    }
}

/// Split `host[:port]` honouring bracketed IPv6 literals. See
/// `crate::h2_proxy::split_host_port` for the same shape used by the
/// listener-level check; duplicated here to keep `h2_to_h1.rs`
/// independent of the proxy module.
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = s.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            let host_with_brackets = &s[..=end + 1];
            let rest = &s[end + 2..];
            let port = rest.strip_prefix(':');
            return (host_with_brackets, port.filter(|p| !p.is_empty()));
        }
        return (s, None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_rejects_authority_host_disagreement() {
        let bridge = H2ToH1Bridge;
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":scheme".into(), "https".into()),
                (":authority".into(), "victim.example".into()),
                ("host".into(), "attacker.example".into()),
            ],
            body: bytes::Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let err = bridge.bridge_request(&req).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("RFC 9113"), "got: {msg}");
    }

    #[test]
    fn bridge_accepts_matching_authority_host() {
        let bridge = H2ToH1Bridge;
        let req = BridgeRequest {
            method: "GET".into(),
            uri: "/".into(),
            headers: vec![
                (":method".into(), "GET".into()),
                (":path".into(), "/".into()),
                (":authority".into(), "example.test:8443".into()),
                ("host".into(), "example.test:8443".into()),
            ],
            body: bytes::Bytes::new(),
            scheme: None,
            trailers: Vec::new(),
        };
        let out = bridge.bridge_request(&req).unwrap();
        let host = out
            .headers
            .iter()
            .find(|(k, _)| k == "host")
            .map(|(_, v)| v.as_str());
        assert_eq!(host, Some("example.test:8443"));
    }
}
