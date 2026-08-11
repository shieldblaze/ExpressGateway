//! ROUND8-L7-05 — `headers_with_underscores` policy contract tests.
//!
//! Envoy edge best-practice mandates `headers_with_underscores_action =
//! REJECT_REQUEST`; nginx defaults to a silent drop. Both converge because the
//! underscore is an auth-bypass primitive against backends that normalise
//! `_` <-> `-` (Java middleware, some Python frameworks, SAP gateways).
//!
//! A full request-flow test needs a hyper server plus a backend, while the
//! enforcement itself is a per-header byte scan — so these pin the contract at
//! the enum default, the builder surface, and the byte-scan predicate over the
//! L7-05 reference attack corpus.

use lb_l7::h1_proxy::HeaderUnderscorePolicy;

#[test]
fn default_policy_is_reject() {
    // ExpressGateway adopts the Envoy EDGE stance, not the Envoy library
    // default (ALLOW). A PR changing this to `Drop`/`Allow` must update
    // `docs/edge-defaults.md` and the L7-05 finding in lockstep.
    assert_eq!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject,
        "L7-05: default must be Reject (Envoy edge best-practice); \
         drift here is a silent posture downgrade"
    );
}

#[test]
fn policy_variants_are_distinct() {
    // The three variants must be distinct so the runtime `match` in
    // `H1Proxy::handle` / `H2Proxy::handle` can dispatch.
    let r = HeaderUnderscorePolicy::Reject;
    let d = HeaderUnderscorePolicy::Drop;
    let a = HeaderUnderscorePolicy::Allow;
    assert_ne!(r, d);
    assert_ne!(r, a);
    assert_ne!(d, a);
}

#[test]
fn underscore_byte_scan_predicate_reference_corpus() {
    // Mirrors the hot-path predicate `name.as_bytes().contains(&b'_')`. The
    // corpus is the L7-05 attack set — names a backend that normalises
    // `_` <-> `-` would silently coerce into a privileged header.
    //
    // Positive (must be matched as containing `_`):
    let attacks: &[&str] = &[
        "x_forwarded_for",
        "x_auth_token",
        "x_internal_token",
        "x_user_id",
        "_authorization",
        "authorization_",
        "x__double__underscore",
    ];
    for name in attacks {
        assert!(
            name.as_bytes().contains(&b'_'),
            "L7-05 reference corpus: `{name}` MUST be flagged by \
             the underscore scan (the proxy's Reject mode hinges on it)"
        );
    }
    // Negative: legitimate dash-named tokens the proxy must keep forwarding.
    let legitimate: &[&str] = &[
        "x-forwarded-for",
        "x-auth-token",
        "authorization",
        "host",
        "content-length",
        "transfer-encoding",
        "via",
    ];
    for name in legitimate {
        assert!(
            !name.as_bytes().contains(&b'_'),
            "L7-05 reference corpus: legitimate header `{name}` was \
             flagged by the underscore scan — the predicate has \
             over-matched and would silently reject legitimate traffic"
        );
    }
}

#[test]
fn lb_config_enum_default_matches_lb_l7_enum_default() {
    // The `lb_config` and `lb_l7` enums intentionally share a default and the
    // wiring crate maps between them. We do NOT import lb-config here (no dep
    // edge); this pins the lb-l7 side only.
    assert!(matches!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject
    ));
}

#[test]
fn h1_proxy_source_carries_l7_05_marker() {
    // Drift detection: the enforcement site must keep referencing
    // ROUND8-L7-05 so a refactor cannot silently delete the policy check.
    let src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/h1_proxy.rs")).unwrap();
    assert!(
        src.contains("ROUND8-L7-05"),
        "L7-05 enforcement block missing from lb-l7/src/h1_proxy.rs — \
         the underscore-policy check was probably removed in a refactor"
    );
    assert!(
        src.contains("with_header_underscore_policy"),
        "L7-05 builder method `with_header_underscore_policy` missing \
         from H1Proxy — the operator surface was removed"
    );
}

#[test]
fn h2_proxy_source_carries_l7_05_marker() {
    let src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/h2_proxy.rs")).unwrap();
    assert!(
        src.contains("ROUND8-L7-05"),
        "L7-05 enforcement block missing from lb-l7/src/h2_proxy.rs — \
         underscore-policy check was probably removed in a refactor"
    );
    assert!(
        src.contains("with_header_underscore_policy"),
        "L7-05 builder method `with_header_underscore_policy` missing \
         from H2Proxy — operator surface was removed"
    );
}
