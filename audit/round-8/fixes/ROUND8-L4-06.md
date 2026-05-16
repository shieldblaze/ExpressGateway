# Plan for ROUND8-L4-06 — Reject `insert_acl_deny(prefix_len = 0)` and `prefix_len > 32`

Finding-ref:     ROUND8-L4-06 (high, Open)
Reference:       Cilium D4 (LPM-trie allowlist; explicit rejection of `prefix_len = 0`); handoff item 7.
Coverage-gap:    Theme 1 (EBPF-2-09 audited Pod padding of the LPM-trie key, "Verified-Fixed"; the prefix-len admission gate was never audited). Userspace simulator (`sim.rs:341`) already documents that `/0` matches everything — the simulator is right; the gate is missing.

Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`                (`insert_acl_deny`: validate `prefix_len`; new error variant)
  - `crates/lb-l4-xdp/tests/round8_acl_admission.rs` (NEW — proof)

Approach:

1. **New error variant** (`crates/lb-l4-xdp/src/loader.rs`):
   ```rust
   #[derive(Debug, thiserror::Error)]
   pub enum XdpLoaderError {
       // existing variants
       #[error("invalid ACL prefix length: got {0}, must be in 1..=32 for IPv4")]
       InvalidAclPrefixV4(u8),
       #[error("invalid ACL prefix length: got {0}, must be in 1..=128 for IPv6")]
       InvalidAclPrefixV6(u8),
   }
   ```
   - Range is `1..=32` (not `0..=32`): `0` is the default-deny
     footgun the finding documents.

2. **Guard in `insert_acl_deny`**:
   ```rust
   pub fn insert_acl_deny(
       &mut self,
       prefix_len: u8,
       ipv4: Ipv4Addr,
   ) -> Result<(), XdpLoaderError> {
       if prefix_len == 0 || prefix_len > 32 {
           return Err(XdpLoaderError::InvalidAclPrefixV4(prefix_len));
       }
       let key = LpmKey::<u32>::new(u32::from(prefix_len), u32::from(ipv4).to_be());
       let mut trie = self.acl_trie()?;
       trie.insert(&key, 1u32, 0).map_err(Into::into)
   }
   ```
   - Belt-and-braces: zero IP at `/32` is permitted (it's a single
     host); only the prefix length is gated.

3. **Symmetric IPv6 guard**: if/when `insert_acl_deny_v6` lands
   (currently absent), the same shape applies with the ceiling `128`.
   Add a `// TODO(L4-06): mirror guard when v6 ACL ships` comment at
   the v6 trie declaration site so the guard cannot be forgotten.

4. **Proof tests** (`crates/lb-l4-xdp/tests/round8_acl_admission.rs`,
   NEW):
   - `reject_prefix_zero`:
     `loader.insert_acl_deny(0, Ipv4Addr::UNSPECIFIED)` returns
     `Err(InvalidAclPrefixV4(0))`. Loud-fail.
   - `reject_prefix_over_32`:
     `loader.insert_acl_deny(33, Ipv4Addr::LOCALHOST)` returns
     `Err(InvalidAclPrefixV4(33))`.
   - `reject_prefix_255`: same with `255` (max `u8`).
   - `accept_prefix_one_through_32`: walk `1..=32`; each succeeds.
   - `accept_host_route_zero_ip_with_full_prefix`:
     `loader.insert_acl_deny(32, Ipv4Addr::UNSPECIFIED)` succeeds —
     only the prefix is the footgun, not the IP itself.
   - Optional sim-side regression: `sim.rs::lpm_trie_zero_prefix_matches_everything`
     gains a sibling test that the userspace API now refuses to
     install such an entry, so the sim's "documented danger" is
     dead-letter and not reachable through public API.

Proof:

- `cargo test -p lb-l4-xdp --test round8_acl_admission`.
- No eBPF change → no verifier-log re-capture needed.

Risk / blast radius:

- API change: `insert_acl_deny` already returns `Result`; the new
  variant is additive. Callers that currently `?`-propagate get the
  new error transparently. No source-level breakage in-tree.
- An operator who legitimately wants "deny everything" was relying
  on `prefix_len = 0` — but the finding's premise is that such an
  install was always a footgun (default-deny breaks all traffic).
  If a legitimate "default-deny + per-IP allowlist" feature is
  needed, that's an *allowlist* LPM-trie, not the *deny* trie —
  out of scope.

Cross-ref:
- EBPF-2-09 (Pod padding for LpmTrieKey): status remains
  Verified-Fixed; the byte-layout fix is correct. This finding
  closes a different surface (admission gate vs. wire-layout).
- The userspace simulator (`crates/lb-l4-xdp/src/sim.rs`) is correct
  and stays unchanged; only the *public API surface* is gated.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
