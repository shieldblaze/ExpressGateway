//! HTTP/2 → HTTP/1.1 bridge: pseudo-headers back to a request line + `Host`.
//!
//! SEC-2-01 runs `check_h2_downgrade` BEFORE the H1 request line is
//! materialised: a malformed H2 frame can still carry hop-by-hop headers, and
//! once the H1 line exists the upstream parser sees them — a desynced response
//! queue is one hop away.

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// Hop-by-hop headers barred from a forwarded H1 response. `pub(crate)` so the
/// STREAMING H1←H2 relay strips the SAME set as the buffering bridge.
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

        for (k, v) in &req.headers {
            match k.as_str() {
                ":method" => method.clone_from(v),
                ":path" => uri.clone_from(v),
                ":scheme" => { /* Dropped in HTTP/1.1 -- scheme is implicit. */ }
                ":authority" => authority = Some(v.clone()),
                _ if k.starts_with(':') => {
                    // Unknown pseudo-header: dropped, not forwarded.
                }
                _ => {
                    regular_headers.push((k.to_lowercase(), v.clone()));
                }
            }
        }

        // SEC-2-01 — run the detector on the REGULAR headers only, AFTER the
        // pseudo-headers were extracted: on the raw list it over-fires, since
        // `check_h2_downgrade` treats any `:`-prefixed name as a smuggle
        // attempt. Fires BEFORE the H1 request line is materialised.
        lb_security::SmuggleDetector::check_all(&regular_headers, /* is_h2_origin = */ true)
            .map_err(|e| L7Error::BridgeError(format!("h2->h1 downgrade smuggle: {e}")))?;

        // An empty `:authority` would produce an invalid empty `Host`.
        let auth = authority
            .filter(|a| !a.is_empty())
            .ok_or_else(|| L7Error::MissingPseudoHeader(":authority".to_owned()))?;

        // PROTO-2-01 / RFC 9113 §8.3.1: a mismatch is host-confusion smuggling
        // against backends that authorise on `Host` (the bridge would otherwise
        // silently replace it). Duplicated from `H2Proxy::handle` because the
        // bridge is a separate entry point.
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
            // Drop the existing Host so the inserted one is the sole entry.
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
                !RESPONSE_HOP_BY_HOP.contains(&lower.as_str())
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

/// PROTO-2-01 — compare `:authority` against `Host` (RFC 9113 §8.3.1 + RFC 3986
/// §3.2.2). Ports compare only when both are explicit; an empty host rejects.
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

/// Split `host[:port]`, IPv6-bracket aware. Duplicated from
/// `crate::h2_proxy::split_host_port` to keep this module proxy-independent.
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
