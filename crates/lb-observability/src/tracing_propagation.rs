//! REL-2-07: W3C trace-context (`traceparent` / `tracestate`)
//! propagation helpers.
//!
//! `proto`, `sec`, and the L7 crates call into this module at every
//! L7 entry/exit point to extract the inbound trace context and to
//! inject a refreshed context onto outbound requests. The full OTLP
//! exporter is feature-gated (`otlp`); the propagation helpers are
//! always available so the hot path keeps the parser even when OTLP
//! is off.
//!
//! Wire format reference: <https://www.w3.org/TR/trace-context/>
//!
//! Format (version-00):
//!   `00-<trace-id 32 hex>-<parent-id 16 hex>-<flags 2 hex>`
//!
//! Span-name convention (locked here so emitters across crates agree):
//!   - L7 inbound request:        `lb.l7.<proto>.request`
//!   - L7 outbound (upstream):    `lb.l7.<proto>.upstream`
//!   - L4 connection:             `lb.l4.<proto>.conn`
//!   - Upstream connect:          `lb.upstream_dial`
//!   - Upstream first byte:       `lb.upstream_first_byte`
//!
//! `<proto>` is one of `h1`, `h2`, `h3`, `quic`, `tcp`, `ws`, `grpc`.

/// W3C `traceparent` header name (lower-case canonical form).
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// W3C `tracestate` header name. We pass tracestate byte-for-byte
/// after a length check; we do not parse the inner vendor fields.
pub const TRACESTATE_HEADER: &str = "tracestate";

/// Maximum allowed `tracestate` length per the W3C spec (§3.3.1.1).
pub const TRACESTATE_MAX_LEN: usize = 512;

/// Configured tracing exporter knob. Populated from
/// `[observability].otlp_endpoint` at startup; an absent endpoint
/// disables OTLP entirely while leaving the propagation helpers
/// active (so we still extract / re-inject headers even when we are
/// not sending spans anywhere).
#[derive(Clone, Debug, Default)]
pub struct OtlpConfig {
    /// OTLP/gRPC endpoint, e.g. `http://otel-collector:4317`. `None`
    /// disables export but keeps in-process propagation.
    pub endpoint: Option<String>,
    /// Parent-based ratio sampler. `1.0` = always; `0.0` = never;
    /// any value in `[0.0, 1.0]` is honoured.
    pub sampler_ratio: f64,
}

impl OtlpConfig {
    /// Default sampler is 1% with the endpoint disabled.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            endpoint: None,
            sampler_ratio: 0.01,
        }
    }
}

/// Decoded W3C trace-context. `flags` is a bitfield — bit 0 is the
/// `sampled` flag; other bits are reserved.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    /// 16-byte trace id, big-endian. All-zero is invalid per spec.
    pub trace_id: [u8; 16],
    /// 8-byte parent span id, big-endian. All-zero is invalid.
    pub parent_id: [u8; 8],
    /// Flags. Use [`TraceContext::sampled`] for the well-known bit.
    pub flags: u8,
}

impl TraceContext {
    /// `true` if the `sampled` bit (bit 0) is set in `flags`.
    #[must_use]
    pub const fn sampled(&self) -> bool {
        (self.flags & 0x01) != 0
    }

    /// Encode back to the wire-format `traceparent` header value
    /// (version `00`).
    #[must_use]
    pub fn to_header(&self) -> String {
        let mut s = String::with_capacity(55);
        s.push_str("00-");
        for b in self.trace_id {
            // hex with no_alloc — two chars per byte.
            s.push(hex_nibble(b >> 4));
            s.push(hex_nibble(b & 0x0f));
        }
        s.push('-');
        for b in self.parent_id {
            s.push(hex_nibble(b >> 4));
            s.push(hex_nibble(b & 0x0f));
        }
        s.push('-');
        s.push(hex_nibble(self.flags >> 4));
        s.push(hex_nibble(self.flags & 0x0f));
        s
    }
}

const fn hex_nibble(n: u8) -> char {
    match n & 0x0f {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'a',
        11 => 'b',
        12 => 'c',
        13 => 'd',
        14 => 'e',
        _ => 'f',
    }
}

/// Parse a `traceparent` header value. Returns `None` on **any**
/// deviation from the version-`00` wire format — callers must
/// forward the original header bytes byte-for-byte even when parsing
/// fails, per W3C §3.2 (forward-compatibility).
#[must_use]
pub fn parse_traceparent(value: &str) -> Option<TraceContext> {
    // Format: 2-32-16-2 = 52 hex chars + 3 dashes = 55.
    if value.len() != 55 {
        return None;
    }
    let bytes = value.as_bytes();
    // Anchor checks via `.get()` so the index-bounds lint stays
    // happy and a too-short input falls through to `None` rather
    // than panicking.
    if bytes.get(2) != Some(&b'-') || bytes.get(35) != Some(&b'-') || bytes.get(52) != Some(&b'-') {
        return None;
    }
    let version = decode_hex_byte(bytes.get(0..2)?)?;
    if version == 0xff {
        // Per spec, `ff` is forbidden as version.
        return None;
    }
    if version != 0 {
        // Future versions are forwarded unchanged (handled by the
        // caller); we only consume `00`.
        return None;
    }
    let mut trace_id = [0u8; 16];
    for (i, chunk) in bytes.get(3..35)?.chunks_exact(2).enumerate() {
        if let Some(slot) = trace_id.get_mut(i) {
            *slot = decode_hex_byte(chunk)?;
        }
    }
    if trace_id.iter().all(|&b| b == 0) {
        return None;
    }
    let mut parent_id = [0u8; 8];
    for (i, chunk) in bytes.get(36..52)?.chunks_exact(2).enumerate() {
        if let Some(slot) = parent_id.get_mut(i) {
            *slot = decode_hex_byte(chunk)?;
        }
    }
    if parent_id.iter().all(|&b| b == 0) {
        return None;
    }
    let flags = decode_hex_byte(bytes.get(53..55)?)?;
    Some(TraceContext {
        trace_id,
        parent_id,
        flags,
    })
}

fn decode_hex_byte(b: &[u8]) -> Option<u8> {
    let hi = decode_hex_nibble(*b.first()?)?;
    let lo = decode_hex_nibble(*b.get(1)?)?;
    Some((hi << 4) | lo)
}

const fn decode_hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Generic header-bag interface so the helpers can call into hyper's
/// `HeaderMap`, h2's `HeaderMap`, or h3's `HeaderMap` without
/// depending on any single one. Each L7 crate provides its own thin
/// adapter at the call site.
pub trait HeaderBag {
    /// Read the first value associated with `name`.
    fn get_first(&self, name: &str) -> Option<&str>;
    /// Append (do NOT replace) a header. Idempotent on the caller —
    /// callers should `remove` first when overwriting is intended.
    fn append(&mut self, name: &str, value: &str);
    /// Remove every value associated with `name`.
    fn remove(&mut self, name: &str);
}

/// Extract the inbound trace context from a header bag, returning
/// both the parsed context and (independently) the raw header bytes
/// so the caller can forward unchanged if parsing fails.
#[must_use]
pub fn extract_parent<H: HeaderBag + ?Sized>(headers: &H) -> ExtractedContext<'_> {
    let traceparent_raw = headers.get_first(TRACEPARENT_HEADER);
    let tracestate_raw = headers
        .get_first(TRACESTATE_HEADER)
        .filter(|v| v.len() <= TRACESTATE_MAX_LEN);
    let parsed = traceparent_raw.and_then(parse_traceparent);
    ExtractedContext {
        parsed,
        traceparent_raw,
        tracestate_raw,
    }
}

/// Result of [`extract_parent`]. Holds borrowed references back into
/// the original header bag.
#[derive(Copy, Clone, Debug)]
pub struct ExtractedContext<'a> {
    /// Parsed [`TraceContext`] if the inbound `traceparent` was valid.
    pub parsed: Option<TraceContext>,
    /// Raw `traceparent` bytes for byte-for-byte forwarding when
    /// parsing fails (per W3C §3.2).
    pub traceparent_raw: Option<&'a str>,
    /// Raw `tracestate` bytes after length check (per W3C §3.3.1.1).
    pub tracestate_raw: Option<&'a str>,
}

/// Inject a `traceparent` (and optionally `tracestate`) into an
/// outbound header bag. Existing values are replaced; the caller is
/// responsible for honouring the W3C "always update" rule.
pub fn inject_into<H: HeaderBag + ?Sized>(
    headers: &mut H,
    ctx: &TraceContext,
    tracestate: Option<&str>,
) {
    headers.remove(TRACEPARENT_HEADER);
    headers.append(TRACEPARENT_HEADER, &ctx.to_header());
    if let Some(ts) = tracestate {
        if ts.len() <= TRACESTATE_MAX_LEN {
            headers.remove(TRACESTATE_HEADER);
            headers.append(TRACESTATE_HEADER, ts);
        }
    }
}

/// Span-name builder shared across L4 / L7 to keep the convention
/// in one place.
///
/// ```
/// use lb_observability::tracing_propagation::span_name;
/// assert_eq!(span_name("l7", "h1", "request"), "lb.l7.h1.request");
/// ```
#[must_use]
pub fn span_name(layer: &str, proto: &str, kind: &str) -> String {
    format!("lb.{layer}.{proto}.{kind}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Minimal in-memory header bag implementing [`HeaderBag`].
    #[derive(Default)]
    struct TestHeaders(HashMap<String, Vec<String>>);

    impl HeaderBag for TestHeaders {
        fn get_first(&self, name: &str) -> Option<&str> {
            self.0
                .get(&name.to_ascii_lowercase())
                .and_then(|v| v.first())
                .map(String::as_str)
        }
        fn append(&mut self, name: &str, value: &str) {
            self.0
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push(value.to_owned());
        }
        fn remove(&mut self, name: &str) {
            self.0.remove(&name.to_ascii_lowercase());
        }
    }

    #[test]
    fn parse_canonical_traceparent() {
        let raw = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let ctx = parse_traceparent(raw).expect("valid traceparent");
        assert!(ctx.sampled());
        assert_eq!(ctx.flags, 0x01);
        // Trace-id last 4 bytes
        assert_eq!(&ctx.trace_id[12..], &[0x1c, 0x80, 0x31, 0x9c]);
    }

    #[test]
    fn rejects_invalid_traceparent_shapes() {
        assert!(parse_traceparent("").is_none());
        assert!(parse_traceparent("garbage").is_none());
        // Wrong delimiter positions.
        assert!(
            parse_traceparent("00x0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").is_none()
        );
        // Version 0xff forbidden.
        assert!(
            parse_traceparent("ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").is_none()
        );
        // All-zero trace id.
        assert!(
            parse_traceparent("00-00000000000000000000000000000000-b7ad6b7169203331-01").is_none()
        );
        // All-zero parent id.
        assert!(
            parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01").is_none()
        );
    }

    #[test]
    fn roundtrip_encode_decode() {
        let raw = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let ctx = parse_traceparent(raw).unwrap();
        assert_eq!(ctx.to_header(), raw);
    }

    #[test]
    fn extract_and_inject_round_trip() {
        let mut inbound = TestHeaders::default();
        inbound.append(
            TRACEPARENT_HEADER,
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        );
        inbound.append(TRACESTATE_HEADER, "vendor1=value1");

        let extracted = extract_parent(&inbound);
        let parsed = extracted.parsed.expect("parses");
        assert!(parsed.sampled());
        assert_eq!(extracted.tracestate_raw, Some("vendor1=value1"));

        let mut outbound = TestHeaders::default();
        inject_into(&mut outbound, &parsed, extracted.tracestate_raw);
        assert_eq!(
            outbound.get_first(TRACEPARENT_HEADER),
            Some("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"),
        );
        assert_eq!(
            outbound.get_first(TRACESTATE_HEADER),
            Some("vendor1=value1")
        );
    }

    #[test]
    fn tracestate_overlong_is_dropped() {
        let mut headers = TestHeaders::default();
        let huge = "v=".to_owned() + &"x".repeat(TRACESTATE_MAX_LEN + 1);
        headers.append(TRACESTATE_HEADER, &huge);
        headers.append(
            TRACEPARENT_HEADER,
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
        );
        let ex = extract_parent(&headers);
        // Length cap defends downstream OTel collectors that limit
        // tracestate to 512 bytes — drop the oversize value.
        assert!(ex.tracestate_raw.is_none());
        // traceparent still parses.
        assert!(ex.parsed.is_some());
    }

    #[test]
    fn span_name_format_is_locked() {
        assert_eq!(span_name("l7", "h1", "request"), "lb.l7.h1.request");
        assert_eq!(
            span_name("l4", "tcp", "conn"),
            "lb.l4.tcp.conn",
            "L4 conn span name",
        );
    }

    #[test]
    fn otlp_disabled_keeps_sampler_ratio() {
        let cfg = OtlpConfig::disabled();
        assert!(cfg.endpoint.is_none());
        assert!((cfg.sampler_ratio - 0.01).abs() < f64::EPSILON);
    }
}
