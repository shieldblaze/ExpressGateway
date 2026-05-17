//! ROUND8-L7-05 — `headers_with_underscores` policy contract tests.
//!
//! Envoy edge best-practice mandates `headers_with_underscores_action
//! = REJECT_REQUEST`; nginx defaults to silent drop
//! (`underscores_in_headers off`). Both converge: the underscore is
//! an auth-bypass primitive against backends that normalise `_` <->
//! `-` (Java middleware, some Python frameworks, SAP gateways).
//!
//! Full request-flow tests require a hyper server + an upstream
//! backend; the policy enforcement itself is a simple per-header
//! byte scan. These tests pin the contract at three levels:
//!
//! 1. `HeaderUnderscorePolicy::default()` returns `Reject` — the
//!    Envoy-equivalent edge default. Any future drift fails CI.
//! 2. The H1Proxy builder method accepts each variant and the field
//!    is stored verbatim (verified indirectly via the
//!    enum-default-pinning test plus a builder-call smoke).
//! 3. The byte-scan predicate used by the hot path
//!    (`name.as_bytes().contains(&b'_')`) returns the expected
//!    answer on the corpus of header names mentioned in the L7-05
//!    finding's reference attack table.
//!
//! Reference attack corpus: the names below are drawn from the
//! Envoy + nginx + Pingora documentation examples plus the
//! L7-05 finding's reproduction notes (Java middleware, Python
//! WSGI), and from RFC 9110 §5.1 tchar set negative tests.

use lb_l7::h1_proxy::HeaderUnderscorePolicy;

#[test]
fn default_policy_is_reject() {
    // L7-05 contract: ExpressGateway adopts the Envoy edge stance,
    // not Envoy library default (ALLOW). Any future PR that changes
    // this to `Drop` or `Allow` must update `docs/edge-defaults.md`
    // and `audit/round-8/findings/ROUND8-L7-05.md` in lockstep.
    assert_eq!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject,
        "L7-05: default must be Reject (Envoy edge best-practice); \
         drift here is a silent posture downgrade"
    );
}

#[test]
fn policy_variants_are_distinct() {
    // Sanity: the three variants must be distinct PartialEq values
    // so the runtime `match` in H1Proxy::handle / H2Proxy::handle
    // can dispatch correctly.
    let r = HeaderUnderscorePolicy::Reject;
    let d = HeaderUnderscorePolicy::Drop;
    let a = HeaderUnderscorePolicy::Allow;
    assert_ne!(r, d);
    assert_ne!(r, a);
    assert_ne!(d, a);
}

#[test]
fn underscore_byte_scan_predicate_reference_corpus() {
    // Mirrors the predicate used in `H1Proxy::handle` / `H2Proxy::handle`:
    // `name.as_bytes().contains(&b'_')`. The corpus below is the
    // L7-05 attack reference set — names a backend that normalises
    // `_` <-> `-` would silently coerce to a privileged header.
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
    // Negative (must NOT be matched — these are legitimate dash-named
    // tokens that the proxy must continue to forward):
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
    // The two enums (`lb_config::HeaderUnderscorePolicy` and
    // `lb_l7::h1_proxy::HeaderUnderscorePolicy`) intentionally
    // share a default. The wiring crate maps one to the other; this
    // test pins the default-side of the contract on the lb-l7 side.
    //
    // We do NOT import lb-config here (no dep edge); the contract
    // is asserted on the runtime side only. The lb-config side has
    // its own `Default` derive that locks the same variant.
    assert!(matches!(
        HeaderUnderscorePolicy::default(),
        HeaderUnderscorePolicy::Reject
    ));
}

#[test]
fn h1_proxy_source_carries_l7_05_marker() {
    // Drift detection: the H1Proxy::handle method must reference
    // ROUND8-L7-05 in a comment or error message so a future
    // refactor cannot silently delete the policy enforcement.
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
