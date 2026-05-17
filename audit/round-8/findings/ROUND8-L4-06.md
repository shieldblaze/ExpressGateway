### ROUND8-L4-06 — `insert_acl_deny` accepts `prefix_len = 0` (default-deny footgun, Cilium D4)

Reference: `audit/round-8/research/cilium.md` D4 (LPM-TRIE allowlist) + handoff item 7
Our equivalent: `crates/lb-l4-xdp/src/loader.rs:801-812` (`insert_acl_deny`)

Severity: high
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — loader.rs rejects prefix_len==0 || >32 with InvalidAclPrefixV4 at function entry before any trie insert; /0-takedown footgun closed for insert_acl_deny. Proof 6/6. See audit/round-8/verify/l4.md.

Divergence:
- Cilium and the handoff are explicit: userspace MUST reject an LPM-trie ACL insert with `prefix_len = 0`. A `0.0.0.0/0 deny` entry installed in `ACL_DENY_TRIE` causes every packet to be dropped at the XDP stage — full LB takedown.
- Us: `insert_acl_deny` takes `prefix_len: u8` and passes it directly to `LpmKey::<u32>::new(u32::from(prefix_len), ...)`. No `if prefix_len == 0 { return Err(...) }` guard. There's no validation that `prefix_len <= 32` either (any `u8 > 32` would be passed through; LPM-trie's behaviour for an over-spec'd prefix is implementation-defined and typically clamped, but worth not relying on).

Impact:
- A single typo in operator tooling — `insert_acl_deny(0, "0.0.0.0".parse().unwrap())` instead of `insert_acl_deny(32, "1.2.3.4".parse().unwrap())` — produces a global deny. The BPF `handle_ipv4` path consults the trie unconditionally (line 412-416 of `ebpf/src/main.rs`); every packet hits `STAT_DROP` and `XDP_DROP`.
- A misbehaving CRD reconciler or a partial YAML deserialisation default that initialises `prefix_len = 0` triggers the same. The default-deny is hard to spot in metrics (it would show `xdp_packets_total{result="drop"}` growing to 100% of traffic).
- The Round-7 audit's existing review touched ACL via EBPF-2-09 (Pod padding) but not the `prefix_len=0` admission gate.

Reproduction:
- Add a one-line unit test in `crates/lb-l4-xdp/src/loader.rs` that calls `loader.insert_acl_deny(0, Ipv4Addr::UNSPECIFIED)` and verifies it errors. Currently it would succeed.
- The `sim.rs` test `lpm_trie_zero_prefix_matches_everything` (line 341) actually documents this exact behaviour — that the userspace simulator denies every address when `/0` is installed. The simulator is correct; the gate is missing.

Recommendation:
1. In `insert_acl_deny`:
   ```rust
   pub fn insert_acl_deny(&mut self, prefix_len: u8, ipv4: Ipv4Addr) -> Result<(), XdpLoaderError> {
       if prefix_len == 0 {
           return Err(XdpLoaderError::Map(MapError::SyscallError(/* or new variant */)));
       }
       if prefix_len > 32 {
           return Err(...);
       }
       // existing body
   }
   ```
   Prefer a new error variant: `XdpLoaderError::InvalidAclPrefix(u8)` for clarity.
2. Add a regression test that asserts the rejection.
3. Symmetric guard for any future `insert_acl_deny_v6` (would be `prefix_len > 128`, with `0` rejected).
