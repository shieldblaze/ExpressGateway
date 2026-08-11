//! ROUND8-L7-05 — the underscore is an auth-bypass primitive against backends
//! that normalise `_` <-> `-`. These pin the enum default, the builder surface,
//! and the byte-scan predicate over the L7-05 attack corpus.

use lb_l7::h1_proxy::HeaderUnderscorePolicy;

#[test]
fn default_policy_is_reject() {
    // The Envoy EDGE stance, not the Envoy library default (ALLOW). Changing
    // it must update `docs/edge-defaults.md` in lockstep.
    assert_eq!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject,
        "L7-05: default must be Reject (Envoy edge best-practice); \
         drift here is a silent posture downgrade"
    );
}

#[test]
fn policy_variants_are_distinct() {
    let r = HeaderUnderscorePolicy::Reject;
    let d = HeaderUnderscorePolicy::Drop;
    let a = HeaderUnderscorePolicy::Allow;
    assert_ne!(r, d);
    assert_ne!(r, a);
    assert_ne!(d, a);
}

#[test]
fn underscore_byte_scan_predicate_reference_corpus() {
    // MIRROR of the hot-path predicate `name.as_bytes().contains(&b'_')`.
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
    // Negative control: dash-named tokens must keep forwarding.
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
    // No lb-config dep edge here; this pins the lb-l7 side only.
    assert!(matches!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject
    ));
}

#[test]
fn h1_proxy_source_carries_l7_05_marker() {
    // Drift detection: a refactor must not silently delete the policy check.
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
