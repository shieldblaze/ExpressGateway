//! HTTP/3 to HTTP/2 bridge (pass-through).
//!
//! HTTP/3 and HTTP/2 share the same pseudo-header scheme, so this bridge
//! performs only lowercase normalization on regular headers.

use crate::{Bridge, BridgeRequest, BridgeResponse, L7Error, Protocol, check_header_count};

/// Pass-through bridge for HTTP/3 to HTTP/2.
pub struct H3ToH2Bridge;

impl Bridge for H3ToH2Bridge {
    fn bridge_request(&self, req: &BridgeRequest) -> Result<BridgeRequest, L7Error> {
        check_header_count(req.headers.len())?;

        let headers: Vec<(String, String)> = req
            .headers
            .iter()
            .map(|(k, v)| {
                if k.starts_with(':') {
                    (k.clone(), v.clone())
                } else {
                    (k.to_lowercase(), v.clone())
                }
            })
            .collect();

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

        let headers: Vec<(String, String)> = resp
            .headers
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v.clone()))
            .collect();

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
        Protocol::Http2
    }
}
