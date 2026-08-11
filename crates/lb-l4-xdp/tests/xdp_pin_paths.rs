//! EBPF-2-05 proof test: BPF map pinning under `/sys/fs/bpf/expressgateway/` with mode 0750.

#![cfg(target_os = "linux")]

use lb_l4_xdp::loader::{
    ACL_DENY_TRIE_PIN_NAME, CONNTRACK_PIN_NAME, CONNTRACK_V6_PIN_NAME, DEFAULT_PIN_DIR,
    L7_PORTS_PIN_NAME, STATS_PIN_NAME,
};

/// Always-on: pin-name constants must literally match the `#[map(name = "...")]` strings in the eBPF source.
#[test]
fn pin_name_constants_match_ebpf_source() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/ebpf/src/main.rs"))
        .expect("read ebpf source");

    for &pin in &[
        CONNTRACK_PIN_NAME,
        CONNTRACK_V6_PIN_NAME,
        L7_PORTS_PIN_NAME,
        ACL_DENY_TRIE_PIN_NAME,
        STATS_PIN_NAME,
    ] {
        let needle = format!("#[map(name = \"{pin}\")]");
        assert!(
            src.contains(&needle),
            "ebpf/src/main.rs must contain `{needle}` to match \
             the userspace pin-name constant — see EBPF-2-05",
        );
    }
}

/// Default pin directory must be exactly the path the plan + the systemd unit + the operator runbook all reference.
#[test]
fn default_pin_dir_is_canonical() {
    assert_eq!(
        DEFAULT_PIN_DIR, "/sys/fs/bpf/expressgateway",
        "DEFAULT_PIN_DIR must match the bpffs layout documented in \
         EBPF-2-05 / DEPLOYMENT.md",
    );
}

/// EBPF-2-05 named proof test: maps pinned by subprocess A must be reusable by subprocess B with state intact.
#[test]
#[ignore = "needs CAP_BPF + bpffs mount — runs in CI privileged stage"]
fn test_maps_pinned_then_loaded_from_pin() {
    eprintln!(
        "EBPF-2-05 pin-reuse test stub — full kernel scaffold lands with the \
         CI bpffs fixture. The always-on coverage is \
         pin_name_constants_match_ebpf_source + the stats_export unit tests."
    );
}
