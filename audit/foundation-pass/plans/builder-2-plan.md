# builder-2 — consolidated Phase-2 plan (tasks #10–#14)

Author: builder-2. Verifier: a DIFFERENT agent (author != verifier, R5).
Branch: audit/foundation-pass. One commit per finding, no push (lead pushes).
Targeted builds/tests only (no `--all-features --workspace` — contends).

Owned, independent of builder-1. Order = #10 → #11 → #12 → #13 → #14.

---

## ITEM 1 — F-COR-2 / BL-1 (task #10, THE R1 BLOCKER)

File: `tests/balancer_counter_sync.rs` (test only; NO product change).

### Proof the assertion is unsound (recorded in commit msg)
`test_no_divergence_under_load` lines 74–91 do THREE non-atomic loads at
t1<t2<t3: `pre=active_connections()`, `sync_from_state()` (snapshot =
atomic@t2), `post=active_connections()`, then assert
`snapshot ∈ [min(pre,post), max(pre,post)]`. That containment bracket is
valid ONLY for a MONOTONIC counter. The counter is non-monotonic: 4 inc +
4 dec threads churn a saturating `AtomicU64` concurrently
(`backend.rs:107` AcqRel `fetch_add`, `:117-129` saturating
`compare_exchange_weak` loop; `sync_from_state` `lib.rs:126-140` is a
single `.load()` copy). The t2 value can legitimately rise above / dip
below both bracket endpoints and return between t1 and t3. Decisive
captured counter-example (auditor-1): `snapshot=8650 bracket=[8649,8649]`
with `pre==post==8649` — a single AtomicU64 cannot tear; 8650 is a
*coherent* value the live counter genuinely held and oscillated away
from. Divergence is bidirectional (3 below / 2 above across the 5/80
repro) = oscillation fingerprint, not a one-directional staleness/order
bug. Product is correct; the assertion asserts a mathematically false
property. Removing it is NOT test-weakening (R5): it is deleting a
provably-unsound assertion that produces FALSE failures.

### Change
1. DELETE the concurrent mid-flight 3-read bracket block (lines ~74–91:
   the `while` loop body's `pre_atomic`/`post_atomic`/`assert!` bracket).
   KEEP the loop only as a churn-driver if needed for the post-join
   check, OR drop the sampling loop and keep the existing **sound**
   post-join exact-equality check (lines 99–104:
   `backend.sync_from_state(); assert_eq!(active_connections,
   final_atomic)`), which is the real CODE-2-14 contract and is retained
   unchanged + still fails if product drifts.
2. ADD a new `#[test] fn snapshot_tracks_atomic_monotonic_midflight()`:
   single-threaded, inc-only N times; after each `inc_connections()`
   capture `pre = active_connections(); sync_from_state(); post =
   active_connections();` and assert `snapshot == pre == post` (in a
   single thread the three reads are exact) AND assert the snapshot is
   monotonically non-decreasing across iterations AND within `[pre,post]`.
   This restores sound mid-flight coverage (the bracket invariant IS
   valid here) and still FAILS if `sync_from_state` ever returned a stale
   / wrong value or the atomic regressed.
3. Keep `with_state_seeds_snapshot_from_atomic` and
   `legacy_backend_keeps_plain_u64_semantics` untouched.

### Test design / regression proof
- Pre-fix mechanism re-demo: run the existing binary under 24× CPU
  oversubscription ≥80 runs to reproduce the false failure (auditor-1
  harness) — documents the unsound assertion is what fails.
- Post-fix: same 24× oversubscription ≥80 runs → 0 failures
  (non-monotonic false-failure gone); the new monotonic sub-test passes;
  the post-join exact-equality still passes.
- Negative guard for the NEW sub-test: temporarily stub
  `sync_from_state` to return stale (local scratch, reverted) → new test
  MUST fail. (Demonstrated, not committed.)

Risk: the post-join exact-equality is already present and sound; the new
sub-test is deterministic (single thread) so zero added flake.

---

## ITEM 2 — F-COR-3 + F-COR-4 (task #11, serialized, same file)

File: `tests/reload_zero_drop.rs`. Commit F-COR-3 then F-COR-4
separately (two commits) since they are distinct findings, same file.

### F-COR-3 — unique per-iteration temp dirs (mirror S3 9e58bbf2)
- `test_sigterm_drains_h2_with_goaway_async` (:921): replace
  `std::env::temp_dir().join("eg-drain-h2")` + `create_dir_all` with
  `unique_temp_dir("drain-h2")` (helper already `create_dir_all`s
  internally); keep the matching `remove_dir_all(&dir)` (:1027).
- `test_sigterm_drains_h3_with_connection_close` (:1062): same with
  `unique_temp_dir("drain-h3")`; keep `remove_dir_all` (:1103).
  Both are in `mod drain_tests` which `use super::drain::*;` so
  `unique_temp_dir` is in scope — confirmed.
- `test_reload_zero_drop_under_load` (:61) is at FILE top level, OUTSIDE
  `mod drain`. `unique_temp_dir` is `pub` inside `mod drain`. Fix:
  reference it as `drain::unique_temp_dir("reload-soak")` (the `drain`
  mod is a sibling at file scope, `pub` fn → reachable as
  `crate`-relative `drain::unique_temp_dir`). Replace
  `temp_dir().join("eg-test-zero-drop")` + `create_dir_all`; keep
  `remove_dir_all` (:113). No assertion/timing change — pure isolation
  hardening mirroring 9e58bbf2.
- Regression proof: pure isolation; demonstrate by `cargo test -p
  lb-integration-tests --test reload_zero_drop` (targeted) green ×3, and
  show the three tests now write `eg-drain-h2-<pid>-<nanos>-<seq>` style
  unique paths (eprintln or strace-free: assert path uniqueness by
  reading the created dir name in a scratch check). The defect class
  (fixed shared path deletable by a concurrent sibling) is structurally
  removed; mirrors the already-verified H1 fix.

### F-COR-4 — gate FinOnly on observed clean EOF + negative regression
`reload_zero_drop.rs:681-693`: replace `let _ = clean_eof;` with a real
gate. New `CloseKind` variant or outcome:
- Add `CloseKind::FinUnverified` (or reuse `None` → a NEW hard-fail
  outcome). Plan: extend `close_kind` classification:
  - `byte_complete && has_conn_close` → `Header`
  - `byte_complete && !has_conn_close && clean_eof` → `FinOnly`
  - `byte_complete && !has_conn_close && !clean_eof` → NEW
    `CloseKind::BodyCompleteNoClose` (a real failure: body complete but
    socket never FIN-closed on drain).
- The per-iteration assertion in
  `test_sigterm_drains_h1_with_connection_close` (:866-873) currently
  accepts `Some(Header) | Some(FinOnly)`. Tighten to MUST be
  `Header | FinOnly` and explicitly FAIL on
  `Some(BodyCompleteNoClose)` with a clear message.
- Read window is 12 s ≫ jitter+drain so a real clean FIN is reliably
  observed (auditor-1: historical FinOnly raw_len=4174 always had Ok(0)).
- Negative regression: add a `#[test]` (or in-test sub-path) using a
  local stub TCP server that writes a complete byte-identical
  HTTP/1.1 200 + body then HOLDS the socket open (never FIN) past the
  read window; feed it through the same `close_kind` classifier
  (extract the classification into a pure fn
  `classify_close(byte_complete, has_conn_close, clean_eof) ->
  Option<CloseKind>` so it is unit-testable). Assert it yields
  `BodyCompleteNoClose` and that the per-iter assertion would FAIL on
  it. This proves the test now catches the "complete body but never
  closed" drain defect it claims to guard.
- Pre/post: pre-fix the negative case is misclassified `FinOnly`
  (PASS, wrong); post-fix it is `BodyCompleteNoClose` (FAIL, correct).
  Real product drain still passes (it does FIN; product unchanged).

Risk: must not turn a legitimately-slow-but-clean FIN into a false
fail — mitigated by the 12 s window already proven generous; the gate
only trips when Ok(0) was genuinely never observed within that window.

---

## ITEM 3 — F-COR-5 + F-COR-8 (task #12, independent files)

Two commits.

### F-COR-5 — remove dead proptest gate (mirror S1 25d8ad84)
File `crates/lb-h3/tests/proptest_qpack.rs`: delete line 15
`#![cfg(feature = "proptest")]`; replace with the same explanatory
doc-comment S1 used on the lb-quic sibling (cite: proptest is an
unconditional dev-dep `Cargo.toml:17`, the `proptest=[]` marker feature
`:21` gates no code; S1 25d8ad84 removed the identical gate from
`lb-quic/tests/proptest_header.rs`). Case logic byte-identical; keep the
PROPTEST_CASES budget.
- Regression: `cargo test -p lb-h3 --test proptest_qpack` (DEFAULT
  features) pre-fix → `running 0 tests`; post-fix → `running 4 tests`
  all pass. Also `cargo test -p lb-h3 --all-features --test
  proptest_qpack` still 4 pass (no regression of the gate path).

### F-COR-8 — per-test atomic counter for h3_graceful_close nonce
File `crates/lb-quic/tests/h3_graceful_close.rs:71-80`: add
`static CERT_SEQ: AtomicU64 = AtomicU64::new(0);` + `let seq =
CERT_SEQ.fetch_add(1, Ordering::Relaxed);` and suffix cert/key paths
`-{seq}` exactly like `round8_h3_authority_enforced.rs:75-81`. Keep the
existing pid*K+subsec nonce, append `-{seq}`. Mechanical hardening; no
behaviour change.
- Regression: `cargo test -p lb-quic --test h3_graceful_close` green;
  inspection shows path now `lb-quic-proto-2-11-cert-{nonce}-{seq}.pem`
  — collision-free by construction even if a 2nd #[test] is added
  (the latent re-introduction auditor-4 flagged is structurally
  closed). (No live failure today; this is cheap latent hardening per
  F-COR-8 LOW/latent.)

---

## ITEM 4 — F-COR-7 (task #13, sec-adjacent + reg test)

File `crates/lb-l4-xdp/src/nic_compat.rs`.

### Mechanism
`ethtool -i ens5` emits an EMPTY `firmware-version:` → `parse_ethtool_
firmware` returns `None` → `firmware_of` → `Err(FirmwareUnresolved)` →
`drv_supported` `:282-286` `Err(_) => return Ok(DrvSupport::Allowed)`
i.e. FAIL-OPEN. The ENA blocklist row can never fire on real AWS ENA.

### Fix (fail-CLOSED on empty/unparseable firmware for fragile drivers)
- Introduce a set of "fragile drivers" (drivers that have a blocklist
  row AND are known to under-report firmware) — at minimum `ena`
  (empirically empty fw on AWS), keyed by driver name from the
  BLOCKLIST. For these, an UNRESOLVED firmware must NOT fail-open.
- In `drv_supported`: when `driver_of` succeeds and the driver has a
  blocklist row, and `firmware_of` returns Err / empty → return
  `DrvSupport::Refuse { reason }` (fail-CLOSED: skip Drv, fall to Skb)
  with a reason explaining "firmware unresolved on a fragile driver
  (ena) — refusing native XDP rather than fail-open; ROUND8-L4-05".
  Drivers WITHOUT a blocklist row keep fail-open (virtual/dummy/veth
  unaffected — `unknown_driver_allowed` test still passes).
- Keep `classify` pure and unchanged (unit tests intact). Add the
  fragile-driver logic in `drv_supported` (the resolution path) so the
  existing pure-function tests are untouched.

### Regression test (exercises the FULL resolution path, not classify())
Add an integration test (privileged-tolerant, but does NOT require
privilege — it only reads sysfs/ethtool) that calls
`drv_supported("ens5")` on THIS box (real ENA, empty fw) and asserts the
result is NOT `Allowed` — i.e. it is `Refuse{..}` (fail-closed), proving
the dead path is now live and no longer fail-opens. Guard with a skip if
the iface is not `ena` (so it is portable / CI-safe): resolve
`driver_of("ens5")`; if `Ok("ena")` assert `Refuse`, else `eprintln
SKIP` (the box here IS ena → it runs for real). Also add a unit-level
test of the new `drv_supported` branch via a small seam: a
`classify_with_unresolved_firmware(driver)` helper or test the public
`drv_supported` against a synthetic iface is not possible (real
sysfs) — so the real ens5 path IS the regression test (auditor-3
explicitly asked for a real `drv_supported("ens5")`-style full-path
test, not classify() directly). Pre-fix: returns `Allowed`
(fail-open) → test FAILS. Post-fix: `Refuse` → test PASSES.

Risk: must not fail-closed on healthy NICs that legitimately report
firmware (mlx5 reports it; ena on AWS does not). Scope the fail-closed
strictly to (driver has blocklist row) AND (firmware unresolved/empty)
— a NIC with a blocklist row but readable safe firmware stays Allowed.

---

## ITEM 5 — F-DOC-1 + F-ESC-1 (task #14)

### F-DOC-1 — doc alignment (two commits or one; docs only)
- `DEPLOYMENT.md` §"Kernel floor" (line 36): currently "5.15 LTS or
  6.1 LTS". Align to reality: effective floor ~5.15 (no ringbuf, no
  kfuncs, BTF/CO-RE loads fine; verified live on 7.0 — D-1). State the
  supported window AND explicitly declare 7.x status: "validated live
  on 7.0 (D-1 native ENA attach); 7.x is NOT YET in the official
  verifier matrix — see open R7 item."
- `audit/ebpf/verifier-logs/README.md` (the "verifier README"):
  matrix is 5.15/6.1/6.6; add a note that the audit box runs 7.0
  (outside the matrix), the effective floor is ~5.15, and that
  "extend the official verifier matrix to 7.x" is an OPEN R7 PRODUCT
  DECISION (not gate-blocking, recorded, not asterisked).
- No code/script change (verify-xdp.sh kver map left as-is; extending
  it is the R7 product call).

### F-ESC-1 — REAL eBPF verifier baseline on kernel 7.0
- Capture a REAL kernel-7.0 verifier baseline by loading
  `crates/lb-l4-xdp/src/lb_xdp.bin` via the PROVEN aya path (the d1
  privileged loader) with `EbpfLoader::verifier_log_level(
  VerifierLogLevel::VERBOSE | STATS)`. Method: add a privileged
  `#[ignore]` capture test in `crates/lb-l4-xdp/tests/` (sibling of
  xdp_attach_mode) that:
  1. loads + `kernel_load`s the `xdp` prog (real BPF_PROG_LOAD on the
     running 7.0 kernel),
  2. reads the loaded program's REAL kernel verifier-derived facts via
     aya `ProgramInfo`: `verified_instruction_count()` (kernel
     `verified_insns`), `size_translated()`, `size_jitted()`,
     `tag()`, `name()`, prog id — these are genuine kernel verifier
     outputs, not a placeholder,
  3. writes a structured real baseline to
     `audit/ebpf/verifier-logs/7.0.log.committed` (kernel uname,
     verified_insns, xlated/jited sizes, prog tag, GPL license
     assertion, capture method, timestamp). bpftool/libbpf cannot load
     the legacy-map ELF (auditor-3 tooling note) so the aya path is the
     ONLY real loader — documented in the baseline file.
  4. ALSO capture the verbose verifier LOG TEXT: aya 0.13.1 only
     surfaces the verifier log on the ERROR path. To get the full
     verbose log on a SUCCESSFUL load I will additionally run a raw
     `bpf(BPF_PROG_LOAD, log_level=2)` via the kernel by re-using
     aya's relocated program bytes IF reachable; if aya 0.13.1 does
     not expose post-reloc instructions publicly, capture the
     authoritative kernel verifier STATS line via `bpftool prog show
     id <id> --json` (verified_insns, jited, xlated) after the aya
     load — bpftool can INSPECT an already-loaded prog even though it
     cannot LOAD this ELF. That yields a real per-7.0 verifier baseline
     (counters are the load-bearing audit artefact per the README
     "Expected content": processed-insns / states counters + verdict).
  - Replace ONLY the 7.0 file (new file; the 5.15/6.1/6.6 placeholders
    are addressed below). Do NOT asterisk (R4).
- 5.15 / 6.1 / 6.6: attempt via lvh/qemu/Docker. Findings: NO lvh, NO
  qemu, NO vng on this box; only Docker is present, and
  `scripts/verify-xdp.sh` requires `--privileged` Docker + lvh-images
  (pinned digests are PLACEHOLDERS in-script — `IMAGE_BASE`/`LVH_IMAGE_*`
  are literal "Placeholder — replace with quay.io/...@sha256"). I will
  ATTEMPT a privileged Docker lvh boot for 5.15; if the lvh image
  digests are placeholders / unreachable (no pinned image to pull) it
  is genuinely infeasible IN THIS ENVIRONMENT. In that case I will
  capture the WHY VERBATIM (exact script output, missing image
  digests, no lvh binary, no qemu) into
  `audit/foundation-pass/messages/builder-2-to-lead-fesc1.md` and
  message lead to ESCALATE the residual 5.15/6.1/6.6 multi-kernel CI
  lane per R6/R7(a) as a scoped CI-infra workstream (~0.5d: pin real
  lvh-images digests + wire the privileged CI matrix stage). NOT
  asterisked (R4) — disposition is "7.0 real-captured this session;
  5.15/6.1/6.6 escalated-with-mechanism".

---

## Sequencing & commits
1. F-COR-2 (test) — 1 commit. [R1 BLOCKER — first]
2. F-COR-3 (reload temp dirs) — 1 commit.
3. F-COR-4 (FinOnly gate + neg reg) — 1 commit. (serialized after #2)
4. F-COR-5 (proptest gate) — 1 commit.
5. F-COR-8 (cert nonce counter) — 1 commit.
6. F-COR-7 (ENA fail-closed + ens5 reg test) — 1 commit.
7. F-DOC-1 (docs) — 1 commit.
8. F-ESC-1 (7.0 baseline + escalation msg if needed) — 1 commit.

Each: targeted build + targeted tests; demonstrate regression
fails-pre / passes-post; commit
`git -c user.email=build@local -c user.name=builder-2 commit`; NO push;
checkpoint message to lead per item; TaskUpdate #10–#14.

REQUESTING: "approved" before any source edit (R5).
