# Batched low / info-severity plans — ebpf area
Format: one short block per ID; each block ends with an explicit
disposition (FIX / DEFER) and the lead-approval line.

---

## EBPF-2-06 — Regression test that `XdpLinkId` drop is safe under aya 0.13.1
Finding-ref:    EBPF-2-06 (low, Open) — confirmed safe against aya
                source; this is a tripwire for future aya upgrades.
Files touched:
  - `crates/lb-l4-xdp/tests/xdp_link_drop.rs`  (NEW)
Approach:       Integration test, gated on `#[cfg(target_os = "linux")]`
                and `#[ignore]` (needs CAP_BPF + CAP_NET_ADMIN).
                1. Create a `dummy0` netdev via rtnetlink.
                2. Load `lb_xdp.bin` via `XdpLoader`.
                3. Call `loader.attach("lb_xdp", "dummy0",
                   XdpMode::Skb)`; the implementation drops the
                   returned `XdpLinkId` (already does today).
                4. Assert attachment is live: query
                   `Xdp::info().nr_attached >= 1` or shell out to
                   `bpftool net show dev dummy0` and grep.
                5. Drop the whole `XdpLoader`.
                6. Assert attachment is gone (count `== 0`).
                If aya 0.14 ever changes the link-ownership model
                so that dropping the `XdpLinkId` detaches, step 4
                fails and the test surfaces the regression before
                production does.
Proof:          The test itself. Name:
                `lb-l4-xdp/tests/xdp_link_drop.rs::xdp_link_persists_after_id_drop`
                Invariants: step-4 attach-count >= 1; step-6
                attach-count == 0.
Risk:           None — a test-only addition. Worst case is a
                flaky CI step on a system without `dummy0` support,
                which is mitigated by `#[ignore]` defaulting.
Cross-ref:      Round-2 cross-review §E-3 (severity confirmed at
                low). Coupled with EBPF-2-07's CI image so the
                same matrix runs this test too.
Disposition:    FIX in Round 4 (one-file, low-risk).
Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
