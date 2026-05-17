# Plan for ROUND8-L4-01 — Reserve `backend_idx = 0` / zero-IP sentinel + `STAT_BACKEND_UNPOPULATED`

Finding-ref:     ROUND8-L4-01 (high, Open)
Reference:       Katran lesson 10 (CH-ring-position-0 reserved as uninitialised, `increment_ch_drop_real_0()`); handoff item 1.
Coverage-gap:    Theme 1 (sentinel-class defenses never audited; Katran lesson never imported). Pillar 4a graded sentinel-free; Round-7 EBPF-2-03 (LRU swap) addressed eviction but never the "zero is a valid backend" admission gate.

Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`             (add `STAT_BACKEND_UNPOPULATED = 10`; insert sentinel guard after CT hit in `handle_ipv4` and `handle_ipv6`)
  - `crates/lb-l4-xdp/src/loader.rs`                (`BackendEntry::new` / `BackendEntryV6::new`: reject zero-IP at construction; insert-side guard on `conntrack_map().insert()` wrapper)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (extend `StatSlot` enum with `BackendUnpopulated`; emit `xdp_packets_total{result="backend_unpopulated"}`)
  - `crates/lb-l4-xdp/tests/round8_backend_sentinel.rs`  (NEW — proof tests)

Approach:

1. **eBPF-side sentinel** (`crates/lb-l4-xdp/ebpf/src/main.rs`):
   - Add slot: `const STAT_BACKEND_UNPOPULATED: u32 = 10;`. Slot is free (current ceiling is 9; map is 32-wide). No layout change.
   - In `handle_ipv4`, immediately after the conntrack hit at the CT lookup site (current `STAT_CT_HIT_V4` increment), insert:
     ```rust
     if entry.backend_ip == 0 || entry.backend_port == 0 {
         incr_stat(STAT_BACKEND_UNPOPULATED);
         return Ok(xdp_action::XDP_PASS);
     }
     ```
   - In `handle_ipv6` (mirror site after `STAT_CT_HIT_V6`):
     ```rust
     if entry.backend_ip == [0u8; 16] || entry.backend_port == 0 {
         incr_stat(STAT_BACKEND_UNPOPULATED);
         return Ok(xdp_action::XDP_PASS);
     }
     ```
   - `XDP_PASS` (not `XDP_DROP`) so a misbehaving controller does not cause silent traffic loss — the kernel's network stack still gets the packet; the counter is the operator signal.

2. **Userspace construction-time guard** (`crates/lb-l4-xdp/src/loader.rs`):
   - Add `XdpLoaderError::BackendUnpopulated { reason: &'static str }` variant.
   - `BackendEntry::new(backend_idx, flags, backend_ip, backend_port, backend_mac, src_mac) -> Result<Self, XdpLoaderError>`:
     - Return `Err` when `backend_ip == 0` or `backend_port == 0`. Construction failure is loud, before any map insert.
   - Same for `BackendEntryV6::new` with `[0u8; 16]` and `0` port.
   - Add `XdpLoader::insert_conntrack_v4(&mut self, key: FlowKey, entry: BackendEntry)` thin wrapper that re-validates the entry (`debug_assert!(entry.backend_ip != 0)` in debug; explicit check in release) before calling `conntrack_map().insert()`. Existing callers can migrate gradually; the raw `conntrack_map()` accessor stays for tests and migration.

3. **stats_export wiring** (`crates/lb-l4-xdp/src/stats_export.rs`):
   - Extend the `StatSlot` enum (which already maps slot indices to Prom label values) with `BackendUnpopulated => "backend_unpopulated"`.
   - The slot iteration loop picks the new entry up automatically.

4. **Proof tests** (`crates/lb-l4-xdp/tests/round8_backend_sentinel.rs`, NEW):
   - `userspace_reject_zero_ip_v4`: `BackendEntry::new(7, 0, 0, 8080, mac, mac)` returns `Err(BackendUnpopulated)`. Same for `port = 0`.
   - `userspace_reject_zero_ip_v6`: `BackendEntryV6::new(7, 0, [0;16], 8080, mac, mac)` returns `Err(BackendUnpopulated)`.
   - `sim_xdp_zero_backend_pass_not_tx`: extend the userspace simulator (`sim.rs`) so a conntrack pre-populated with a zero-IP `BackendEntry` (via the raw test backdoor, bypassing `new`) produces `XDP_PASS` plus an increment of the new stat slot. Asserts both the counter and the action.
   - `stats_slot_label_round_trips`: `StatSlot::BackendUnpopulated.as_str() == "backend_unpopulated"`.

Proof:

- `cargo test -p lb-l4-xdp --test round8_backend_sentinel` passes in CI (no kernel needed; tests run against the userspace simulator).
- After the fix, the existing `xdp_packets_total` metric exposes
  `xdp_packets_total{result="backend_unpopulated"}`; a deployment that
  somehow ends up with a zero-IP CT entry sees this counter tick
  instead of forwarding to `0.0.0.0:0`.

Risk / blast radius:

- The sentinel check adds two `cmp` + `jz` instructions per CT-hit
  branch in the verifier-checked path. Verifier impact: minimal, but
  see ROUND8-L4-10 — the verifier-log capture per kernel matrix
  (5.15 / 6.1 / 6.6) must be re-run after this patch and committed.
- The `BackendEntry::new` signature change from infallible to
  `Result<Self, _>` is a breaking change for any external caller. Audit
  reveals all current callers are in-tree (`loader.rs`, `lib.rs`
  `HotSwapManager`, and tests). Update the call sites in the same
  patch.

Cross-ref:
- ROUND8-L4-04 (atomic backend-table swap): this finding's sentinel is
  the *floor* defence; L4-04's atomic swap is the deeper fix. Land L4-01
  first since it's a 1-day patch and immediately closes the silent-
  XDP_TX-to-0.0.0.0 vector.
- ROUND8-L4-07 (`BackendEntry::flags` dead code): both findings touch
  `BackendEntry`; coordinate so the L4-07 plan's "drop or use the
  field" decision lands serialised after the L4-01 sentinel.
- ROUND8-L4-10 (verifier-log baseline): re-capture the log after this
  patch; CI gate: `scripts/verify-xdp.sh 5.15 && scripts/verify-xdp.sh 6.1 && scripts/verify-xdp.sh 6.6`.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
