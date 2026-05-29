# SESSION 15 — Independent verify evidence

**Verifier:** verifier (independent of authors)
**Branch:** feature/quic-proxy-s15
**Method:** owner-rulings BINDING; per-increment evidence accreted here.

---

## Phase 0 verify — gate re-run on 6905bede

**Commit verified:** `6905bede3a8154fb3b4921623d21bd359bf1f01b`
**Subject:** `chore(s15 phase0): fix ~85 pre-existing rust-1.95.0 clippy regressions`
**Worktree:** clean before gates; pulled to tip via `git pull --ff-only`.
**Toolchain:** `cargo +stable` resolves to rustc 1.95.0 (59807616e 2026-04-14); cargo 1.95.0. (Note: `rust-toolchain.toml` pins to 1.85; `+stable` channel selector overrides the pin — matches what builder-1 ran. The pin update to 1.95 is a separate carry-forward.)
**CARGO_TARGET_DIR:** `/home/ubuntu/Code/eg-target` (shared with builder-1; coordinated handoff via SendMessage to avoid incremental-cache contention).

### Gate 1 — workspace clippy (-D warnings)

```
cargo +stable clippy --workspace --all-targets --all-features -- -D warnings
```

- **Result:** `Finished dev profile [unoptimized + debuginfo] target(s) in 53.71s`, **EXIT=0**.
- Wall-clock: real 53.826s / user 2m56.193s / sys 0m41.787s.
- Log: `audit/quic/verify-logs/p0-clippy.log`.

### Gate 2 — fmt --check

```
cargo +stable fmt --all -- --check
```

- **Result:** EXIT=0, no diff. Log: `audit/quic/verify-logs/p0-fmt.log` (empty file = clean).

### Gate 3 — workspace test ×1 (--no-fail-fast)

```
CARGO_BUILD_JOBS=4 cargo +stable test --workspace --all-features --no-fail-fast
```

- **Result:** **CARGO_EXIT=0**, **passed=1308 / failed=0 / ignored=18**.
- Wall-clock: real 9m30.960s / user 2m37.523s / sys 0m23.623s.
- Log: `audit/quic/verify-logs/p0-test.log` (1308 `test result:` aggregations across 91+ suites — full lcov-style breakdown preserved).
- **1308 vs S14 baseline 1286:** +22 tests, explained by builder-1's A1 source restore landing in-worktree (public_header lib tests + public_header_differential.rs) which `--workspace` picks up. The 22 deltas land entirely in lb-quic and don't displace any pre-existing suite. R3 no-regression: SATISFIED (zero failures, zero S14-suite shrinkage).
- **CF-SATURATION-1 status:** did NOT reappear at `CARGO_BUILD_JOBS=4`. No need to drop to `--test-threads=1`.
- **First-attempt anomaly:** attempt 1 (background, 25-min harness timeout) was killed mid-test at 91 of ~95 suites with 522 passed / 0 failed before timeout (log preserved at `audit/quic/verify-logs/p0-test-attempt1-truncated.log`). Cause: harness background-task budget < test-execution wall-clock for the two big suites (101s and 245s respectively). Re-ran foreground with 10-min budget — clean completion in 9m31s.

### Gate 4 — code-read of the single new `#[allow]`

**Site:** `tests/ws_proxy_e2e.rs:91-99` (in spawn_echo_backend handler).

```rust
                while let Some(Ok(msg)) = rx.next().await {
                    // clippy::collapsible_match suggests moving
                    // `tx.send(msg).await.is_err()` into a guard, which
                    // rustc rejects: guards cannot move (E0382 — `msg`
                    // would be moved before subsequent arms can match
                    // on it). The if-let-inside-match shape below is
                    // the working form. rust-1.95.0 clippy bug;
                    // revisit on next toolchain bump.
                    #[allow(clippy::collapsible_match)]
                    match msg {
```

- **Finding:** sole new `#[allow]` introduced by 6905bede; carries the rustc-rejection rationale (E0382 — guards cannot move). Lint name is narrow (`collapsible_match` only) — NOT a blanket `#[allow(clippy::all)]` or category-wide disable. Rust-1.95.0 clippy bug acknowledged with revisit-on-bump.
- **Cross-check:** `git diff 36ee1227..6905bede` shows `tests/ws_proxy_e2e.rs` is the only file gaining a `#[allow(clippy::...)]`. The other `#[allow(clippy::*)]` sites grep-ed in the worktree all pre-date 36ee1227 (lb-quic/src/{lib,conn_actor,router,h3_bridge}.rs, lb-balancer, lb-h2, lb-h3, lb-l7/authority, lb/main).
- **Verdict:** PASS — single targeted suppression, justified, not a category disable.

### Gate 5 — semantic-equivalence spot-check on lb-l7 `manual_contains` edits (3 sites)

Edits verified by reading `git diff 36ee1227..6905bede -- crates/lb-l7/src/*.rs`. All transforms are direct semantic equivalences with no allocation or logic change.

**Site 5a — `crates/lb-l7/src/h1_to_h1.rs:57-100` (2 hunks, 4 edits total — both request- and response-leg filter passes):**

```
- if HOP_BY_HOP_HEADERS.iter().any(|h| *h == lower.as_str()) {
+ if HOP_BY_HOP_HEADERS.contains(&lower.as_str()) {
- if conn_named.iter().any(|n| *n == lower) {
+ if conn_named.contains(&lower) {
```

Analysis: `[&str]::contains(&&str)` performs byte-equality comparison element-by-element, identical to `iter().any(|h| *h == X)`. `&lower` borrows the existing String; no clone, no alloc. Filter behaviour bit-for-bit equivalent. PASS.

**Site 5b — `crates/lb-l7/src/h1_to_h2.rs:73-114` (2 hunks, 4 edits — request-leg filter + 1-RTT pseudo-header filter):**

Same transform pattern as 5a, applied to identical HOP_BY_HOP / conn_named lookups. Critical because H1→H2 has trailer-passthrough constraints — no semantic change here means trailer policy is unchanged. PASS.

**Site 5c — `crates/lb-l7/src/h2_to_h1.rs:141` (1 edit — response-leg filter using `RESPONSE_HOP_BY_HOP`):**

```
- !RESPONSE_HOP_BY_HOP.iter().any(|h| *h == lower.as_str())
+ !RESPONSE_HOP_BY_HOP.contains(&lower.as_str())
```

Negated `.any()` → negated `.contains()`. Boolean inversion preserved. PASS.

**Verdict for §5:** all 3 spot-checks (and by extension the matching edits in h1_to_h3.rs, h3_to_h1.rs, h1_proxy.rs, h3_bridge.rs) are PURE refactors. No allocation introduced, no logic change. The clippy lint name `manual_contains` is correctly applied.

### Gate 6 — disk-free post-gate

Post-Gate-3 disk: `25 GB free` on `/` (host `/dev/root` 67G total, 64% used). Meets ≥25GB threshold from task description. Adequate runway for builder-1's upcoming workspace step 4 + Phase 2 A2 build.

### Phase 0 verdict

**PASS.** All six gate criteria from team-lead's task description met:

| Gate | Criterion | Result |
| ---- | --------- | ------ |
| 1    | workspace clippy -D warnings | EXIT=0 in 53.71s |
| 2    | fmt --check clean | EXIT=0, no diff |
| 3    | workspace test ×1 PASS (CARGO_BUILD_JOBS=4) | 1308/0/18 in 9m31s |
| 4    | `#[allow]` site read + rustc-rejection comment | single targeted; PASS |
| 5    | 3 lb-l7 spot-checks for semantic equivalence | h1_to_h1 / h1_to_h2 / h2_to_h1 all pure refactor |
| 6    | ≥25GB disk free post-gate | 25G free; threshold met |

R3 no-regression: zero failed tests; test count +22 over S14 baseline tracks A1 source addition (lb-quic public_header tests). No CF-SATURATION-1 reappearance at `CARGO_BUILD_JOBS=4`.

**Carry-forward observed:** `rust-toolchain.toml` pin (1.85) lags actual gate toolchain (1.95.0 via `+stable`). Not blocking — `+stable` channel selector overrides the pin, but the pin should eventually be bumped so `cargo` without `+stable` runs the same toolchain the gates run. Not a Phase 0 blocker; flag for a future cleanup commit. CF-S15-TOOLCHAIN-PIN-LAG.

Builder-1 cleared to fire step 4 (workspace clippy + fmt + workspace test ×1) as the A1 promote-readiness gate.

---

## Phase 2 A1 verify — SHARED-1 public-header parser on fa1c1a24

**Commits verified:** `c248df45 feat(s15 a1): SHARED-1 quiche-free QUIC public-header parser` + `fa1c1a24 test(s15 a1): bump proptest case budget 256 → 1000 per design §A1` (verify on the bump tip).
**Worktree:** clean, pulled to `fa1c1a24`.
**Toolchain:** `+stable` = rustc 1.95.0; `+nightly` = nightly 1.98.0 (57d06900f 2026-05-27) for llvm-cov.

### Gate 1 — RFC 9001 fixture lib tests

```
cargo +stable test -p lb-quic --lib public_header
```

- **Result:** 19/19 PASS, 28 filtered out, finished in 0.01s (cached compile + cold test). EXIT=0.
- Log: `audit/quic/verify-logs/a1-lib-tests.log`.
- Notable test names verified by lib-suite tail: `rfc9001_a2_initial_parses`, `handshake_parses`, `empty_buffer_is_too_short`, `fixed_bit_clear_long_rejected`, `fixed_bit_clear_short_rejected`, `dcid_too_long_rejected`, `scid_too_long_rejected`, `initial_truncated_in_token_len_varint`, `initial_declared_length_overruns`, `short_header_positive`, `short_header_zero_len_rejected`, `short_header_too_short_for_dcid`, `version_negotiation_overrides_type_bits`, `retry_classified_no_length`, `varint_{one,two,four,eight}_byte`, `varint_incomplete`.
- **Fixture-provenance code-read:**
  - `rfc9001_a2_initial()` at `public_header.rs:396-408` builds the public-header bytes verbatim from RFC 9001 §A.2 (`byte0 = 0xc3`, version = `0x00000001`, dcid_len = `0x08`, dcid = `0x8394c8f03e515708`, scid_len = `0x00`, token_len = `0x00`, length varint = `0x449e` = 1182). The comment block at lines 382-395 cites the section explicitly and explains why §A.2 (unprotected cleartext) is the right wire to test rather than §A.3 (which is header-protected over the SAME public-header bytes — protection masks ONLY byte0 low 4 bits + PN, not the public-header fields the parser reads). This is correct: the design doc cites §A.4 (Initial)/§A.5 (Handshake) at line 214 + 674, but those RFC 9001 sections are actually about Retry and ChaCha20-Poly1305 Short Header respectively. **CF-S15-DESIGN-RFC9001-CITATION** — design doc miscites the section numbers; builder's §A.2 selection realizes the intent correctly. Suggest s15-design.md amend in A4.
  - `handcrafted_handshake()` at `public_header.rs:443-454`: RFC 9001 has no Handshake worked-example, so builder synthesized one from RFC 9000 §17.2.4 (`byte0 = 0xe3`: Long + Fixed + Type=0b10 Handshake + PN-len=2; version `0x00000001`; dcid `0xdeadbeef`; scid `0xcafef00d`; length varint `0x4010` = 16; 16 bytes of pad). Verified against RFC 9000 §17.2.4 — type bits 0b10 = Handshake, version + dcid/scid lengths align.

### Gate 2 — 1000-case differential proptest vs quiche::Header::from_slice

```
cargo +stable test -p lb-quic --test public_header_differential
```

- **Result:** 3/3 PASS, finished in **17.19s**. EXIT=0. Log: `audit/quic/verify-logs/a1-differential.log`.
- Property tests in `crates/lb-quic/tests/public_header_differential.rs`:
  1. `initial_diff_matches_quiche` (1000 cases): mints client Initial via real `quiche::connect()` for `scid_len ∈ 0..=20`, parses with both `parse_public_header` and `quiche::Header::from_slice`, asserts bit-for-bit equality on `(ty, version, dcid, scid, token)`. Direct realization of design §A1 verify-gate 2.
  2. `initial_length_within_packet_bounds` (1000 cases): round-trip length check — quiche-emitted Initial total = `n` bytes; our declared `length` is asserted `> 0` and `≤ pkt.len()`. This is the "option b" length differential from lead's addendum 3 (the relevant addendum, distinct from the budget number).
  3. `ours_never_panics` (1000 cases): `catch_unwind` on `parse_public_header` over random `u8` vectors `0..200` × `short_dcid_len ∈ 0..=21`. No-panic regression net.
- **Budget verified:** `crates/lb-quic/tests/public_header_differential.rs:128` reads `cases: 1000`; `max_global_rejects: 4096`. Matches design §A1 line 676 binding. (See [[s15-a1-proptest-budget-discrepancy]] memory — earlier 256 was unapproved silent regression caught in pre-read, fixed by builder in fa1c1a24.)
- Wall-clock 17.19s independently reproduces builder's reported 17.11s — same test runs identically.

### Gate 3 — clippy -D warnings, scoped lb-quic

```
cargo +stable clippy -p lb-quic --all-targets --all-features -- -D warnings
```

- **Result:** EXIT=0 in 0.44s (cached). Log: `audit/quic/verify-logs/a1-clippy.log`.
- **Crate-deny posture (code-read at `crates/lb-quic/src/lib.rs:63-71`):**
  ```rust
  #![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
          clippy::indexing_slicing, clippy::todo, clippy::unimplemented,
          clippy::unreachable, missing_docs)]
  ```
  `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::match_wildcard_for_single_variants))]` — note: cfg(test) allows the panic-trio for in-`tests` modules but **NOT `indexing_slicing`**, so the parser body remains under the indexing deny even from inline test fixtures.
- **Parser body audit (`crates/lb-quic/src/public_header.rs:135-358`):**
  - Zero `.unwrap()`, zero `.expect()`, zero `panic!()`, zero `unreachable!()`.
  - Only `.unwrap_or(...)` (infallible default) used on lines 296, 297, 308, 309, 345 — bounded fallbacks for `slice::get(range)` returning empty + `usize::try_from(u64)` returning u8::MAX. Verified semantically: each fallback is the safe "treat as empty/maximum" path, not a hidden panic.
  - All indexing via `.get(i)?` or `.get(range)?` (lines 136-358) with `?` propagating `HeaderError::TooShort{..}` on out-of-bounds. No `pkt[i]` or `pkt[a..b]` in the parse path.
  - All arithmetic via `.checked_add(...)` (lines 219, 238, 281, 348) returning `Err(HeaderError::TooShort)` on overflow.
  - No `unsafe` blocks in `public_header.rs`.
- Differential test file has `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` + `#![allow(clippy::indexing_slicing)] // test-only fixtures` (`public_header_differential.rs:18-19`). Appropriate scope: deny still binds the production-shaped logic; the allow is bounded to the integration-test fixture file.
- Verdict: **PASS** — no new unsafe/unwrap/expect/panic/indexing_slicing landed; the crate-deny posture is preserved.

### Gate 4 — SHARED-1 invariant doc-block (code-read)

**Doc-block:** `public_header.rs:1-29` — module-doc opens with:

> SHARED-1: QUIC public-header parser. Quiche-free, no_alloc on the parse path, no panics, no decryption.
>
> INVARIANT: this parser reads ONLY the cleartext public-header bytes (form, version, DCID-len/DCID, SCID-len/SCID, token-len/token for Initial, length-varint where present). It NEVER touches encrypted payload, packet-number bytes, or header-protected reserved bits. The Mode A no-decrypt property is LAYERED on top of this guarantee — verifier code-read will check this is upheld.
>
> Note on header protection (RFC 9001 §5.4): HP masks byte0's low 4 bits (Long: reserved + PN-len; Short: reserved + key-phase + PN-len) plus the encrypted packet-number bytes. The fields this parser reads (form bit, fixed bit, type bits 4-5 on Long, version, DCID length+bytes, SCID length+bytes, token, length-varint) are NOT header-protected — they are wire-cleartext.

- **Verifier code-read against the body:**
  - `parse_public_header` reads `b0` only — uses `b0 & 0x40` (fixed bit, NOT protected) and `b0 & 0x80` (form bit, NOT protected) for branch. Lines 170-181.
  - `parse_long` reads bytes 0-5 (form + fixed + reserved-but-cleartext type bits 4-5 + version 4 bytes BE + dcid_len) then `dcid` + `scid_len` + `scid`. Lines 186-248. **Does NOT read bits 0-3 of byte0** (which are header-protected per RFC 9001 §5.4.1: reserved bits 2-3 + PN-len bits 0-1). Verified: the type extraction at line 262 masks `(b0 >> 4) & 0x03` — bits 4-5 only, which RFC 9001 §5.4.1 lists as part of the cleartext public header.
  - `parse_long` Initial path reads token-len varint, token bytes, length varint. Lines 273-304. The length varint's value is recorded but NEVER consumed beyond bounds-checking — the parser does NOT touch the AEAD-protected payload past the length-varint.
  - `parse_short` reads byte0 (with form bit check at the top-level entry) + caller-supplied dcid_len bytes. Lines 338-359. **Does NOT** read byte0 bits 0-2 (header-protected in short: K + PN-len). Bit 5 (S = spin bit) is also wire-cleartext per RFC 9000 §17.3.1.
  - No call site reads `pkt[after_dcid..]` for short headers or beyond `length`-bytes for long headers. The parse path is a strict prefix.
- **Verdict:** Invariant doc-block is **accurate and load-bearing** — the body upholds the documented guarantee. This is the type-2 (state-level) prong of the NEVER-DECRYPTED proof; A2 will add the type-3 cargo-bloat linkage prong + the runtime kprobe cross-check.

### Gate 5 — CF-S15-VARINT-SINGLESOURCE marker

**Site:** `public_header.rs:129-130`:

```rust
// SHARED-V — RFC 9000 §16 varint. Mirror of lb_h3::decode_varint by
// intent; consolidate into lb-quic-codec in S16 (CF-S15-VARINT-SINGLESOURCE).
```

- **Verdict:** Present with the exact shape lead's task description specified. Comment names the carry-forward identifier (`CF-S15-VARINT-SINGLESOURCE`) so future grep finds the consolidation TODO; the S16 destination crate (`lb-quic-codec`) is named so the future single-source lives in a versioned target.
- **Differential vs `lb_h3::varint::decode_varint`:** intentionally not unified in S15 because lb-h3 is a sibling crate and pulling it as a dep into lb-quic for one function would create a dep-graph cycle in the consolidation direction A2 needs. S16 plan (per design §7) creates the shared codec crate that both depend on. Mirror-by-intent is acceptable.

### Gate 6 — llvm-cov ≥85% session-scope on public_header.rs

```
cargo +nightly llvm-cov -p lb-quic --lib --no-fail-fast --summary-only
cargo +nightly llvm-cov -p lb-quic --lib --no-fail-fast --lcov --output-path …
```

**Summary readout (`audit/quic/verify-logs/a1-llvm-cov-summary.log`):**

| File              | Regions | Cover  | Functions | Cover  | Lines  | Cover  |
| ----------------- | ------- | ------ | --------- | ------ | ------ | ------ |
| public_header.rs  | 642     | 91.43% | 26        | 96.15% | 375    | **93.60%** |

**Lcov DA per-line cross-check (per [[llvm-cov-session-scope-method]] method, lead-binding):**

Lcov export at `audit/quic/verify-logs/a1-llvm-cov.lcov`. Extracted `SF:.../public_header.rs … end_of_record` block and counted DA / FN / FNDA lines:
- `LF=375, LH=351 → 93.60% lines` (matches summary; whole-file ≡ session-scope here since the file IS the session)
- `FNF=26, FNH=25 → 96.15% functions` (matches summary)
- 23 missed DA lines, 1 missed function (mangled `parse_long0` = the `|_| HeaderError::Truncated { ... }` closure on lines 276-279 for token_len > usize::MAX — by-design defensive branch impossible to hit on 64-bit)

**Session-scope verdict: 93.60% lines / 96.15% functions, both >> 85% threshold.** PASS.

**Missed-line characterization (transparency, not a finding):**
- 15 of 23 missed lines are `panic!()` calls inside test negative-branches (`panic!("expected TooShort, got {other:?}")` etc.) — lines 431, 476, 484, 494, 504, 516, 529, 540, 561, 575, 584, 595, 621, 643, 681. By construction unreachable when tests pass.
- Genuine parse-path gaps (8 lines):
  - **Line 264** (`0b01 => LongType::ZeroRtt`): the 0-RTT classification arm is not covered by any lib fixture. The differential test mints only Initials (quiche's `connect()` first flight). Cheap add: a positive 0-RTT fixture would close this.
  - **Lines 277-279, 311-314**: `HeaderError::Truncated` defensive branches for usize::try_from overflow (Initial) and declared_length > remaining (0-RTT/Handshake). The Initial overflow branch fires only for token_len > 2^63, impossible on a real wire. The 0-RTT/Handshake truncation branch fires only when declared_len > remaining — the lib Handshake fixture has declared_len == remaining (16==16), so no truncation. Cheap add: a negative Handshake-truncation fixture would close this.

**Gap impact:** ≥85% bar cleared comfortably (93.60%). The two gap classes (no 0-RTT fixture, no Handshake-truncation negative) are flagged as soft carry-forwards but do NOT block A1. **CF-S15-A1-COV-CLOSURE** — optional A2 add of one 0-RTT positive + one Handshake-truncation negative would push cov to ~96%.

### Gate 7 — Cargo bloat / NEVER-DECRYPTED linkage prong

Per task description: **NOT REQUIRED at A1** (no Mode A code paths yet; the cargo-bloat prong of the three-part NEVER-DECRYPTED proof binds at A2 when `passthrough.rs` lands and the `quic-passthrough-only` feature gate appears). **Carried forward** — A2 verify will:
1. Run `cargo bloat -p lb-quic --release --filter quiche` against a `--features quic-passthrough-only --no-default-features` binary and assert zero `quiche::Connection`/BoringSSL symbols on the Mode A path.
2. Code-read `FlowEntry` for absence of key material (this Phase 1 §A1 owner-rulings #5 PRIMARY-2 prong).
3. bpftrace kprobe `openat` cross-check during the A2 real-wire test (CHECK-only, not load-bearing).

### Phase 2 A1 verdict

**PASS.** All seven gate criteria met.

| Gate | Criterion | Result |
| ---- | --------- | ------ |
| 1    | RFC 9001 §A.2 + handcrafted Handshake fixtures | 19/19 lib tests PASS |
| 2    | 1000-case proptest differential vs quiche::Header | 3/3 PASS in 17.19s |
| 3    | No new unsafe / unwrap / expect / panic / indexing_slicing | scoped clippy -D warnings EXIT=0; code-read clean |
| 4    | SHARED-1 invariant doc-block present + accurate | doc-block at lines 1-29; body upholds the guarantee |
| 5    | CF-S15-VARINT-SINGLESOURCE registered with exact shape | line 129-130 |
| 6    | llvm-cov ≥85% session-scope on public_header.rs | **93.60% lines / 96.15% functions** (lcov DA cross-check matches summary) |
| 7    | cargo bloat NEVER-DECRYPTED linkage | DEFERRED to A2 per task description |

**Carry-forwards observed:**
- **CF-S15-DESIGN-RFC9001-CITATION** — `audit/quic/s15-design.md` lines 214 and 674 cite "RFC 9001 §A.4 (Initial), §A.5 (Handshake)"; actual RFC 9001 sections are §A.2 Client Initial and §A.5 ChaCha20-Poly1305 Short Header (no Handshake example). Builder correctly used §A.2 + handcrafted Handshake from RFC 9000 §17.2.4. Suggest design-doc amend in A4 final-report cycle.
- **CF-S15-A1-COV-CLOSURE** — public_header.rs coverage 93.60% clears the bar but a one-line 0-RTT positive fixture (`LongType::ZeroRtt` arm) + a Handshake declared-length-overrun negative fixture would push to ~96%. Optional A2 add.
- **CF-S15-VARINT-SINGLESOURCE** — registered in code at line 130; S16 destination `lb-quic-codec`.

**A1 cleared for promote stack.** Builder-1 may proceed to A2 (passthrough datapath core). Lead's A2 plan-approval (#6) is unblocked from the A1 side.

---

