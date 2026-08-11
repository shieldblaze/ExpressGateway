//! ROUND8-OPS-06 / REL-2-07 — L7 wire-in for the W3C trace-context codec in
//! `lb_observability::tracing_propagation`, which shipped with ZERO L7
//! callsites (hence REL-2-07 sitting at `Verified-Fixed-Partial`).
//!
//! This module is that callsite: a [`HeaderBag`] adapter over hyper's
//! `HeaderMap`, plus [`RequestTrace::open`], which extracts the inbound
//! context, mints a fresh child span-id (W3C §3.2 "always update the
//! parent-id"), opens the request span, and hands back the child context for
//! injection upstream — including the ROUND8-L7-01 WebSocket-upgrade dial.
//!
//! Span-id minting: we are never the trace ROOT on the hot path — when the
//! client omits `traceparent` we synthesise a trace-id once so child-id
//! derivation has a stable anchor. The child-id is a process-startup nonce
//! XOR-folded with a monotonic counter: unique within a process lifetime with
//! no CSPRNG dep edge. Span-ids are NOT a security boundary — the trace-id is
//! the correlation key and it is client/upstream supplied.

use std::sync::atomic::{AtomicU64, Ordering};

use lb_observability::tracing_propagation::{self, ExtractedContext, HeaderBag, TraceContext};

/// Adapter so the W3C codec can operate over hyper's `HeaderMap`. `get_first`
/// parses the first value as UTF-8 (a non-UTF-8 `traceparent` is invalid per
/// W3C §3.2 anyway and falls through to "absent"); `inject_into` calls
/// `remove` then `append` so the "always update" rule holds.
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
    // The codec never mutates through `extract_parent`; keep these total (no
    // panic) so a future codec change degrades to a no-op rather than a crash.
    fn append(&mut self, _name: &str, _value: &str) {}
    fn remove(&mut self, _name: &str) {}
}

/// Process-startup nonce, re-seeded once per process so two replicas minting
/// span-ids for the same inbound trace-id do not collide. Derived from the
/// process start instant + pid, avoiding a `rand`/`getrandom` dep edge.
fn startup_nonce() -> u64 {
    use std::sync::OnceLock;
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let pid = u64::from(std::process::id());
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // splitmix64 finaliser — good avalanche without a crypto dep;
        // uniqueness comes from wall-clock nanos + pid, not unpredictability.
        let mut z = (pid << 32) ^ since;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    })
}

static SPAN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Mint a fresh 8-byte span-id (never all-zero, which the W3C codec
/// rejects). Per-process unique for the process lifetime.
fn mint_span_id() -> [u8; 8] {
    let n = SPAN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut v = startup_nonce() ^ n.rotate_left(17).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    if v == 0 {
        v = 1;
    }
    v.to_be_bytes()
}

/// Synthesised trace-id when the client omits `traceparent` (all-zero is
/// invalid per W3C). This is the only branch where we are the trace ROOT.
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
    /// Opened request span. Callers `.instrument(span)` any spawned
    /// upstream/tunnel work so events nest under it.
    pub span: tracing::Span,
    /// Fresh child context — inject this onto the outbound request so
    /// the upstream sees *our* span as its parent (W3C §3.2).
    pub child: TraceContext,
    /// Raw inbound `tracestate` to forward byte-for-byte (length
    /// already capped by the codec, W3C §3.3.1.1).
    pub tracestate: Option<String>,
}

impl RequestTrace {
    /// Extract the inbound context from `headers`, open the request span, and
    /// derive the child context to inject upstream. `proto` is one of `h1` /
    /// `h2` / `ws` / `grpc`; `method` / `target` / `listener` / `sni` populate
    /// the OTLP-schema span fields the exporter expects.
    /// `http.target`, `net.sni`).
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
            // No inbound context: we are the root. Sample bit on so it exports.
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

        // Child context: same trace-id, OUR span-id as the new parent-id (W3C
        // "always update"), flags carried through.
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

    /// Inject the child `traceparent` (+ forwarded `tracestate`) onto an
    /// outbound request's header map, right before the upstream dial.
    pub fn inject_upstream(&self, headers: &mut hyper::HeaderMap) {
        let mut bag = HyperHeaders(headers);
        tracing_propagation::inject_into(&mut bag, &self.child, self.tracestate.as_deref());
    }

    /// W3C `traceparent` value for the child context, for upstream paths that
    /// build a fresh request — e.g. the tungstenite WS client builder, which
    /// takes header pairs rather than a `HeaderMap`.
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
        // trace-id is preserved verbatim.
        assert_eq!(
            &rt.child.trace_id,
            &[
                0x0a, 0xf7, 0x65, 0x19, 0x16, 0xcd, 0x43, 0xdd, 0x84, 0x48, 0xeb, 0x21, 0x1c, 0x80,
                0x31, 0x9c
            ]
        );
        // parent-id is OUR fresh span-id, NOT the client's verbatim.
        assert_ne!(
            &rt.child.parent_id,
            &[0xb7, 0xad, 0x6b, 0x71, 0x69, 0x20, 0x33, 0x31]
        );
        assert!(rt.child.parent_id.iter().any(|&b| b != 0));
        // sampled flag carried through.
        assert!(rt.child.sampled());
    }

    #[test]
    fn missing_traceparent_synthesises_root() {
        let h = hm(&[]);
        let rt = RequestTrace::open(&h, "h1", "GET", "/", "l", None);
        assert!(rt.child.trace_id.iter().any(|&b| b != 0));
        assert!(rt.child.parent_id.iter().any(|&b| b != 0));
        // synthesised root is sampled so the span is exported.
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
        // same shape, trace-id preserved, parent-id == our span id.
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
