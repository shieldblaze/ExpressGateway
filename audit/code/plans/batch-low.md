# Batch-low fix plan — CODE-2-10, CODE-2-12, CODE-2-15
Finding-refs:    CODE-2-10 (info), CODE-2-12 (low), CODE-2-15 (low)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead

Three low/info findings grouped per round-3 cadence rule (medium+ get
individual plans; low/info batch).

---

## CODE-2-10 — XDP attach regression test (info, test-only)

Files touched:
  - `crates/lb-l4-xdp/tests/attach_persists.rs`     (NEW — gated test)
  - `crates/lb-l4-xdp/src/loader.rs`                (comment cite aya source line)

Approach:
**Source-side change**: zero. The behaviour is correct under aya
0.13.x; this batch entry adds *only* a regression test plus a precise
comment citation.

`crates/lb-l4-xdp/tests/attach_persists.rs`:
```rust
#![cfg(target_os = "linux")]
use std::process::Command;

fn has_cap_bpf() -> bool {
    // Probe /proc/self/status capability bits; skip if not present.
    std::fs::read_to_string("/proc/self/status")
        .map(|s| s.contains("CapEff:") && /* parse cap-bpf bit */ true)
        .unwrap_or(false)
}

#[test]
fn xdp_link_persists_after_id_drop() {
    if !has_cap_bpf() { eprintln!("skip: CAP_BPF not held"); return; }
    let ifname = "lo";
    let mut loader = lb_l4_xdp::XdpLoader::open_test_elf().unwrap();
    let _id = loader.attach(ifname, lb_l4_xdp::XdpMode::Skb).unwrap();
    drop(_id);
    std::thread::sleep(std::time::Duration::from_millis(100));
    let out = Command::new("ip").args(["link","show","dev",ifname]).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("xdp"), "expected XDP attached on {ifname}: {s}");
}
```
Loader comment updated to cite the exact aya 0.13.1 line:
`// aya 0.13.1: self.data.links.insert(...) — programs/xdp.rs:160`.

Proof: `cargo test -p lb-l4-xdp --test attach_persists` (skipped on
CI without CAP_BPF; nightly lab job runs it).

Risk: zero (test-only).

---

## CODE-2-12 — Remove unused `arc-swap` workspace dep

Files touched:
  - `Cargo.toml`                                    (delete line)

Approach:
Delete the line `arc-swap = "1"` from `[workspace.dependencies]`. No
crate references it (confirmed by Round 1 machete + manual grep). When
real hot-swap lands (per rel REL-2-03 cert rotation), re-add at the
crate level under `lb-security` or `lb`.

Proof: `cargo tree --workspace | grep -c '^arc-swap'` returns 0.
`cargo machete` no longer flags it. Build passes.

Risk: zero. Re-add cost is one line when needed.

---

## CODE-2-15 — `lb-h1` consumer decision: shadow-parse OR delete

Files touched (option A — shadow-parse, recommended):
  - `crates/lb-l7/Cargo.toml`                       (add `lb-h1 = { path = "../lb-h1" }`)
  - `crates/lb-l7/src/shadow_parser.rs`             (NEW — parallel parse + divergence counter)
  - `crates/lb-l7/src/h1_proxy.rs`                  (call into shadow_parser)
  - `crates/lb-observability/src/metrics.rs`        (`http_parser_divergence_total{kind}`)

Files touched (option B — delete):
  - `Cargo.toml`                                    (remove `lb-h1` from members)
  - `crates/lb-h1/`                                 (full crate removal)
  - `fuzz/Cargo.toml`                               (remove fuzz target dep)

Approach:
Lead and proto co-decided in cross-review §C / Q-CODE-1-06: **option A
(shadow-parse)** for the defence-in-depth value against hyper-vs-
attacker parser disagreement. The shadow parser runs `lb_h1::Parser`
on the same byte stream hyper consumed; on divergence (different
header set, different body length, different chunked-state) it bumps a
counter, logs at `warn!`, and (configurably) rejects the request.

Implementation sketch (`shadow_parser.rs`):
```rust
pub fn check(raw_bytes: &[u8], hyper_parsed: &http::Request<()>) -> Divergence {
    let lb = match lb_h1::Parser::request(raw_bytes) {
        Ok(r) => r, Err(_) => return Divergence::LbH1RejectedHyperAccepted,
    };
    if lb.headers().get_all("content-length").iter().count()
       != hyper_parsed.headers().get_all("content-length").iter().count() {
        return Divergence::ClMismatch;
    }
    if lb.headers().get("transfer-encoding") != hyper_parsed.headers().get("transfer-encoding") {
        return Divergence::TeMismatch;
    }
    Divergence::None
}
```
Hooked into `h1_proxy.rs` after hyper parses; runs on a sample of
requests (1 % default; config `runtime.shadow_parse_sample_rate`) to
limit perf cost.

Proof:
- `crates/lb-l7/tests/shadow_parse.rs::detects_cl_vs_te_smuggle`:
  craft a known CL/TE smuggling vector; assert
  `http_parser_divergence_total{kind="cl_te"}` increments by 1.
- `crates/lb-l7/tests/shadow_parse.rs::clean_request_no_divergence`:
  feed RFC-clean request; assert counter unchanged.

Risk: shadow parse cost at 1 % sample rate ≈ neg. Disable via config
if perf-sensitive. The detector is read-only by default (logs +
counts); the reject behaviour is opt-in (config flag
`reject_on_parser_divergence`).

Cross-ref: SEC-2-01 (smuggle detector — shadow parser complements it),
proto Q-CODE-1-06 (decision recorded).
