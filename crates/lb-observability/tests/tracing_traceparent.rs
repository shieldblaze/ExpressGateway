//! A `traceparent` must round-trip BYTE-FOR-BYTE when no child span replaces the parent-id.

use std::collections::HashMap;

use lb_observability::tracing_propagation::{
    HeaderBag, TRACEPARENT_HEADER, TRACESTATE_HEADER, TraceContext, extract_parent, inject_into,
    parse_traceparent, span_name,
};

#[derive(Default)]
struct Bag(HashMap<String, Vec<String>>);

impl HeaderBag for Bag {
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

const KNOWN_HEADER: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

#[test]
fn test_traceparent_roundtrip() {
    let mut inbound = Bag::default();
    inbound.append(TRACEPARENT_HEADER, KNOWN_HEADER);
    inbound.append(TRACESTATE_HEADER, "rojo=00f067aa0ba902b7");

    let ex = extract_parent(&inbound);
    let parsed: TraceContext = ex.parsed.expect("known traceparent must parse");
    assert!(parsed.sampled());
    assert_eq!(ex.traceparent_raw, Some(KNOWN_HEADER));
    assert_eq!(ex.tracestate_raw, Some("rojo=00f067aa0ba902b7"));

    let mut outbound = Bag::default();
    inject_into(&mut outbound, &parsed, ex.tracestate_raw);

    // With no child span the forward path MUST be a verbatim pass-through.
    let traceparent_out = outbound
        .get_first(TRACEPARENT_HEADER)
        .expect("traceparent injected");
    assert_eq!(
        traceparent_out, KNOWN_HEADER,
        "wire bytes must round-trip without a child-span rewrite",
    );

    let tracestate_out = outbound
        .get_first(TRACESTATE_HEADER)
        .expect("tracestate injected");
    assert_eq!(tracestate_out, "rojo=00f067aa0ba902b7");

    let reparsed = parse_traceparent(traceparent_out).expect("self-parse");
    assert_eq!(reparsed, parsed);
}

#[test]
fn test_traceparent_with_child_span_rewrite() {
    // W3C continuation invariant: a child span changes ONLY the parent-id segment.
    let parent = parse_traceparent(KNOWN_HEADER).unwrap();
    let child = TraceContext {
        trace_id: parent.trace_id,
        parent_id: [0xc0, 0xff, 0xee, 0x00, 0x00, 0xc0, 0xde, 0x00],
        flags: parent.flags,
    };
    let mut headers = Bag::default();
    inject_into(&mut headers, &child, None);
    let raw = headers.get_first(TRACEPARENT_HEADER).unwrap();
    assert_eq!(&raw[..35], &KNOWN_HEADER[..35], "trace-id must survive");
    assert_eq!(&raw[52..], "-01", "flags must survive");
    assert_eq!(&raw[36..52], "c0ffee0000c0de00");
}

#[test]
fn test_span_name_convention_documented() {
    // Dashboards filter on these strings, so a rename must break CI.
    assert_eq!(span_name("l7", "h1", "request"), "lb.l7.h1.request");
    assert_eq!(span_name("l7", "h2", "request"), "lb.l7.h2.request");
    assert_eq!(span_name("l7", "h3", "request"), "lb.l7.h3.request");
    assert_eq!(span_name("l4", "tcp", "conn"), "lb.l4.tcp.conn");
}
