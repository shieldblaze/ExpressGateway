//! ROUND8-OPS-06 / REL-2-07 — the L7 callsite for the W3C trace-context codec
//! in `lb_observability::tracing_propagation`.
//!
//! Span-ids are a process-startup nonce XOR-folded with a counter: unique per
//! process lifetime with no CSPRNG dep edge. They are NOT a security boundary —
//! the trace-id is the correlation key and it is client-supplied.

use std::sync::atomic::{AtomicU64, Ordering};

use lb_observability::tracing_propagation::{self, ExtractedContext, HeaderBag, TraceContext};

/// Adapter so the W3C codec can operate over hyper's `HeaderMap`; `inject_into`
/// does `remove` then `append` so the W3C "always update" rule holds.
pub struct HyperHeaders<'a>(pub &'a mut hyper::HeaderMap);

impl HeaderBag for HyperHeaders<'_> {
    fn get_first(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .and_then(|v| std::str::from_utf8(v.as_bytes()).ok())
    }

    fn append(&mut self, name: &str, value: &str) {
        if let (Ok(n), Ok(v)) = (
            hyper::header::HeaderName::from_bytes(name.as_bytes()),
            hyper::header::HeaderValue::from_str(value),
        ) {
            self.0.append(n, v);
        }
    }

    fn remove(&mut self, name: &str) {
        if let Ok(n) = hyper::header::HeaderName::from_bytes(name.as_bytes()) {
            self.0.remove(n);
        }
    }
}

/// Read-only header view (for `extract_parent` on a non-mutable bag).
pub struct HyperHeadersRef<'a>(pub &'a hyper::HeaderMap);

impl HeaderBag for HyperHeadersRef<'_> {
    fn get_first(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .and_then(|v| std::str::from_utf8(v.as_bytes()).ok())
    }
    // Total (no panic) so a future codec change degrades to a no-op.
    fn append(&mut self, _name: &str, _value: &str) {}
    fn remove(&mut self, _name: &str) {}
}

/// Process-startup nonce so two replicas minting span-ids for the same inbound
/// trace-id do not collide, without a `rand`/`getrandom` dep edge.
fn startup_nonce() -> u64 {
    use std::sync::OnceLock;
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let pid = u64::from(std::process::id());
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // splitmix64 finaliser: uniqueness comes from wall-clock nanos + pid,
        // NOT from unpredictability.
        let mut z = (pid << 32) ^ since;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    })
}

static SPAN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Mint a fresh 8-byte span-id (never all-zero — the W3C codec rejects that).
fn mint_span_id() -> [u8; 8] {
    let n = SPAN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut v = startup_nonce() ^ n.rotate_left(17).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    if v == 0 {
        v = 1;
    }
    v.to_be_bytes()
}

/// Synthesised trace-id when the client omits `traceparent` — the only branch
/// where we are the trace ROOT (all-zero is invalid per W3C).
fn synth_trace_id(seed: u64) -> [u8; 16] {
    let hi = startup_nonce().wrapping_mul(0xff51_afd7_ed55_8ccd) ^ seed;
    let lo = seed.rotate_left(32).wrapping_mul(0xc4ce_b9fe_1a85_ec53) ^ startup_nonce();
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&hi.to_be_bytes());
    out[8..].copy_from_slice(&lo.to_be_bytes());
    if out.iter().all(|&b| b == 0) {
        out[15] = 1;
    }
    out
}

/// Lower-case hex helper for the span fields.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

/// Per-request trace state: the opened request `tracing::Span` plus the child
/// [`TraceContext`] that must be injected onto the upstream request.
pub struct RequestTrace {
    /// Opened request span; callers `.instrument(span)` spawned work.
    pub span: tracing::Span,
    /// Fresh child context to inject upstream (W3C §3.2 "always update").
    pub child: TraceContext,
    /// Raw inbound `tracestate`, forwarded byte-for-byte (length capped by the
    /// codec, W3C §3.3.1.1).
    pub tracestate: Option<String>,
}

impl RequestTrace {
    /// Extract the inbound context, open the request span, and derive the child
    /// context to inject upstream.
    #[must_use]
    pub fn open(
        headers: &hyper::HeaderMap,
        proto: &str,
        method: &str,
        target: &str,
        listener: &str,
        sni: Option<&str>,
    ) -> Self {
        let bag = HyperHeadersRef(headers);
        let ExtractedContext {
            parsed,
            tracestate_raw,
            ..
        } = tracing_propagation::extract_parent(&bag);

        let span_seed = SPAN_COUNTER.load(Ordering::Relaxed);
        let (trace_id, inbound_parent, flags) = match parsed {
            Some(ctx) => (ctx.trace_id, Some(ctx.parent_id), ctx.flags),
            // Root: sample bit on so the span exports.
            None => (synth_trace_id(span_seed), None, 0x01),
        };
        let span_id = mint_span_id();

        let trace_hex = hex(&trace_id);
        let parent_hex = inbound_parent.map_or_else(String::new, |p| hex(&p));
        let span_hex = hex(&span_id);

        let span = tracing::info_span!(
            "lb.l7.request",
            otel.name = %tracing_propagation::span_name("l7", proto, "request"),
            trace_id = %trace_hex,
            span_id = %span_hex,
            parent_id = %parent_hex,
            http.method = %method,
            http.target = %target,
            net.sni = sni.unwrap_or(""),
            listener = %listener,
            http.status_code = tracing::field::Empty,
        );

        // Same trace-id, OUR span-id as the new parent-id (W3C "always update").
        let child = TraceContext {
            trace_id,
            parent_id: span_id,
            flags,
        };

        Self {
            span,
            child,
            tracestate: tracestate_raw.map(str::to_owned),
        }
    }

    /// Inject the child `traceparent` (+ `tracestate`) before the upstream dial.
    pub fn inject_upstream(&self, headers: &mut hyper::HeaderMap) {
        let mut bag = HyperHeaders(headers);
        tracing_propagation::inject_into(&mut bag, &self.child, self.tracestate.as_deref());
    }

    /// Rendered child `traceparent`, for upstream builders that take header
    /// pairs rather than a `HeaderMap` (e.g. tungstenite).
    #[must_use]
    pub fn child_traceparent(&self) -> String {
        self.child.to_header()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn hm(pairs: &[(&str, &str)]) -> hyper::HeaderMap {
        let mut m = hyper::HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                hyper::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                hyper::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn child_keeps_trace_id_replaces_parent() {
        let raw = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let h = hm(&[("traceparent", raw)]);
        let rt = RequestTrace::open(&h, "h1", "GET", "/x", "lstn", None);
        assert_eq!(
            &rt.child.trace_id,
            &[
                0x0a, 0xf7, 0x65, 0x19, 0x16, 0xcd, 0x43, 0xdd, 0x84, 0x48, 0xeb, 0x21, 0x1c, 0x80,
                0x31, 0x9c
            ]
        );
        // parent-id must be OUR fresh span-id, NOT the client's.
        assert_ne!(
            &rt.child.parent_id,
            &[0xb7, 0xad, 0x6b, 0x71, 0x69, 0x20, 0x33, 0x31]
        );
        assert!(rt.child.parent_id.iter().any(|&b| b != 0));
        assert!(rt.child.sampled());
    }

    #[test]
    fn missing_traceparent_synthesises_root() {
        let h = hm(&[]);
        let rt = RequestTrace::open(&h, "h1", "GET", "/", "l", None);
        assert!(rt.child.trace_id.iter().any(|&b| b != 0));
        assert!(rt.child.parent_id.iter().any(|&b| b != 0));
        assert!(rt.child.sampled());
    }

    #[test]
    fn inject_round_trips_onto_hyper_headers() {
        let raw = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let h = hm(&[("traceparent", raw), ("tracestate", "vendor=1")]);
        let rt = RequestTrace::open(&h, "ws", "GET", "/chat", "l", Some("svc"));
        let mut upstream = hyper::HeaderMap::new();
        rt.inject_upstream(&mut upstream);
        let got = upstream.get("traceparent").unwrap().to_str().unwrap();
        assert!(got.starts_with("00-0af7651916cd43dd8448eb211c80319c-"));
        assert!(!got.contains("b7ad6b7169203331"));
        assert_eq!(
            upstream.get("tracestate").unwrap().to_str().unwrap(),
            "vendor=1"
        );
        assert_eq!(rt.child_traceparent(), got);
    }

    #[test]
    fn span_ids_are_unique_per_call() {
        let h = hm(&[(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        )]);
        let a = RequestTrace::open(&h, "h1", "GET", "/", "l", None);
        let b = RequestTrace::open(&h, "h1", "GET", "/", "l", None);
        assert_ne!(a.child.parent_id, b.child.parent_id);
    }
}
