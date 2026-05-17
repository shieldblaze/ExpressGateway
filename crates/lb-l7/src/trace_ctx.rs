//! ROUND8-OPS-06 / REL-2-07 — L7 wire-in for the W3C trace-context
//! propagation library.
//!
//! `lb_observability::tracing_propagation` shipped the full
//! `traceparent` / `tracestate` codec in author-sha `1d462c7` but had
//! **zero** L7 callsites. The Round-2 reliability review (REL-2-07)
//! flagged this as `Verified-Fixed-Partial` precisely because
//! "library committed" was indistinguishable from "library + callsite
//! committed" in the audit register.
//!
//! This module is that callsite. It provides:
//!
//! 1. A [`HeaderBag`] adapter over hyper's [`hyper::HeaderMap`] so the
//!    codec can read the inbound `traceparent` / `tracestate` and
//!    write a refreshed context onto the outbound (upstream) request
//!    without lb-l7 depending on any single HTTP crate's header type.
//! 2. [`RequestTrace::open`] — extract the inbound context, mint a
//!    fresh child span-id (so the upstream sees *our* span as parent,
//!    per W3C §3.2 "always update the parent-id"), open a
//!    `tracing::info_span!` with the canonical
//!    `lb_observability::tracing_propagation::span_name` vocabulary,
//!    and hand back the child [`TraceContext`] for injection onto the
//!    upstream request (including the WebSocket-upgrade dial — the
//!    ROUND8-L7-01 path).
//!
//! Span-id minting note: we are never the trace *root* on the proxy
//! hot path — when the client omits `traceparent` we synthesise a
//! trace-id once so the child-id derivation has a stable anchor, but
//! the common case is an inbound trace-id that already carries the
//! entropy. The child-id is a process-startup nonce XOR-folded with a
//! monotonic per-process counter; this is unique within a process
//! lifetime without pulling a CSPRNG dep edge onto the L7 hot path
//! (span-ids are not a security boundary — the trace-id is the
//! correlation key and it is client/honoured-upstream supplied).

use std::sync::atomic::{AtomicU64, Ordering};

use lb_observability::tracing_propagation::{self, ExtractedContext, HeaderBag, TraceContext};

/// Adapter so the W3C codec can operate over hyper's `HeaderMap`.
///
/// `get_first` returns the first value parsed as UTF-8 (a non-UTF-8
/// `traceparent` is invalid per W3C §3.2 anyway and falls through to
/// "absent"). `append` / `remove` mutate in place; `inject_into`
/// calls `remove` then `append` so the "always update" rule holds.
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
    // The codec never calls `append` / `remove` through `extract_parent`;
    // these are unreachable for the read path but the trait requires
    // them. Keep them total (no panic) so a future codec change that
    // mutates during extraction degrades to a no-op rather than a crash.
    fn append(&mut self, _name: &str, _value: &str) {}
    fn remove(&mut self, _name: &str) {}
}

/// Process-startup nonce. Re-seeded once per process so two replicas
/// minting span-ids for the same inbound trace-id do not collide. The
/// seed is derived from the process start instant + pid so we avoid a
/// `rand`/`getrandom` dep edge onto the L7 crate.
fn startup_nonce() -> u64 {
    use std::sync::OnceLock;
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let pid = u64::from(std::process::id());
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        // splitmix64 finaliser over (pid<<32 ^ nanos) — good avalanche
        // without a crypto dep; uniqueness across replicas comes from
        // the wall-clock nanos + pid, not unpredictability.
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

/// Synthesised trace-id when the client omits `traceparent`. All-zero
/// is invalid per W3C, so fold the startup nonce + counter into 16
/// bytes. We only become the trace *root* in this branch.
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

/// Hex helper for the span fields (lower-case, no allocation churn on
/// the hot path beyond the single `String`).
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0x0f), 16).unwrap_or('0'));
    }
    s
}

/// The per-request trace state: the request `tracing::Span` (already
/// opened with `trace_id` / `parent_id` fields) plus the child
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
    /// Extract the inbound context from `headers`, open the request
    /// span named `lb.l7.request` (the canonical `otel.name` field
    /// carries the `lb.l7.<proto>.request` vocabulary), and derive
    /// the child context to inject upstream.
    ///
    /// `proto` is one of `h1`, `h2`, `ws`, `grpc` (the
    /// `tracing_propagation::span_name` vocabulary). `method` /
    /// `target` / `listener` / `sni` populate the OTLP-schema span
    /// fields div-ops's exporter expects (`http.method`,
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
            // No inbound context: we are the root. Synthesise a
            // trace-id; sample bit on so the span is exported.
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

        // The child context the upstream must see: same trace-id,
        // OUR span-id as the new parent-id (W3C "always update"),
        // flags carried through.
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

    /// Inject the child `traceparent` (+ forwarded `tracestate`) onto
    /// an outbound request's header map. Used right before the
    /// upstream dial — including the ROUND8-L7-01 WebSocket-upgrade
    /// dial.
    pub fn inject_upstream(&self, headers: &mut hyper::HeaderMap) {
        let mut bag = HyperHeaders(headers);
        tracing_propagation::inject_into(&mut bag, &self.child, self.tracestate.as_deref());
    }

    /// W3C `traceparent` header value for the child context (used by
    /// upstream paths that build a fresh request — e.g. the
    /// tungstenite WS client builder, which takes header pairs rather
    /// than a `HeaderMap`).
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
