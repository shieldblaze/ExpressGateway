# SESSION 33 — Bulk Dependency Upgrade (remaining PR-#222 crates; quiche HELD)

Branch: `feature/bulk-deps-s33` (base: `main` @ `c81f42dc`, S31 promoted).
Toolchain: pinned **1.88** (`rust-toolchain.toml`). Box: 8 cores, ~35 GB free.
Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`.

Roles: lead (coordination/verdict/report), dep-eng (bumps + API adaptations), verifier
(independent binding ×3 + h2spec + WS matrix + re-soak). Author≠verifier per stage (R5).

---

## PHASE 0 — baseline + hygiene + registry-quiche confirm

### Hygiene
- Base tip confirmed: `main` @ `c81f42dc` ("Promote S31: quiche 0.28.0->0.29.1 …"). Branched
  `feature/bulk-deps-s33` off it, pushed to origin.
- `ps aux`: no S32 strays at start; killed a transient nohup clippy job (mis-launched), relaunched
  baseline gate harness-tracked.
- Disk: 35 GB free (≥25 GB OK). Shared target dir 9.3 GB.
- Non-root, no sudo, no `git stash`. No `[patch.crates-io]` anywhere.

### REGISTRY-QUICHE CONFIRM (R9 — must use registry 0.29.1, NOT the S32 patched checkout)
- `Cargo.lock`: `quiche 0.29.1` + `tokio-quiche 0.19.0`, both `source = registry+…crates.io`,
  with checksums (registry, not path/git).
- `grep -rn "quiche-0.29.1-s32|/Code/quiche|[patch" --include=*.toml` → **NONE**. No `[patch]` in
  root `Cargo.toml`; `.cargo/config.toml` only sets `BINDGEN_EXTRA_CLANG_ARGS` (no source override).
- The S32 out-of-repo checkout `/home/ubuntu/Code/quiche-0.29.1-s32` exists on disk but is
  **unreferenced** → build uses registry quiche. ✅ Confirmed.

### sc9 CARRIED BASELINE (the KNOWN quiche `collected` staircase — NOT a new leak)
quiche is HELD this session (collected-leak fix is going UPSTREAM, S32). CF-GRPC-H3-CHURN-RSS WILL
still reproduce in the Phase-4 re-soak and is the documented carried baseline — do NOT read it as a
regression. From S32 (`audit/soak/s32-soak-data/repro-pre.log`, registry quiche 0.29.1, sc9 1800s):

```
=== SOAK sc9_grpc_h3 — DRIFT (finding) (1800s, 121 samples) ===
  rss_kb   [ DRIFT] last-third median 54332 vs first-third 39846 (+36.4%), monotone 100%,
                    slope 367.1/sample — first=8472 last=82120 min=8472 max=82120
  vmhwm_kb [ DRIFT] last=86260
  fds      [BOUNDED] 12 ;  threads [BOUNDED] 9 ;  panic_total [BOUNDED] 0
  grpc_h3_sustained: ok=2974777 err=0    grpc_h3_churn: ok=1987448 err=0
```
Root cause (S32-proven): quiche `StreamMap::collected` unbounded insert-only `HashSet` (LIVE leak,
hblkhd doubling 7→14→28→56MB, uordblks flat). All OTHER soak scenarios (Mode A/B/H3/WS) MUST be
BOUNDED in Phase 4; only sc9 DRIFT is expected/carried.

### Baseline R1 (reference, this box/toolchain)
- fmt: PASS · clippy `--all-targets --all-features -D warnings`: PASS
- test `--workspace --all-features --no-fail-fast` ×3: **GREEN — 1512/0/18 ×3** (exit 0 ×3, zero
  FAILED). Exact match to S31 reference on same commit. Box reproduces green → any post-bump failure
  is attributable to its bump. Logs: `audit/deps/s33-gate-baseline/`.

**PHASE 0 COMPLETE** ✅

### In-scope crate inventory (Cargo.lock, pre-bump)
| crate | current | target | route |
|---|---|---|---|
| http | 1.4.0 | 1.4.1 | cargo update (caret `"1"`) |
| serde_json | 1.0.149 | 1.0.150 | cargo update (`"1"`) |
| libc | 0.2.184 | 0.2.186 | cargo update (`"0.2"`) |
| rustls | 0.23.38 | 0.23.40 | cargo update (`"0.23"`) |
| rustls-pki-types | 1.14.0 | 1.14.1 | cargo update (`"1"`) |
| dashmap | 6.1.0 | 6.2.1 | cargo update (`"6"`) |
| prometheus | 0.13.4 | 0.14.0 | **spec edit** `"0.13"`→`"0.14"` (0.x minor) |
| object | 0.36.7 | 0.37.3 | **spec edit** `"0.36"`→`"0.37"` (0.x minor) |
| hyper | 1.9.0 | 1.10.1 | cargo update (`"1"`) — Phase 3 |
| h2 | 0.4.13 | 0.4.14 | cargo update (`"0.4"`) — Phase 3 |
| tokio-tungstenite | 0.24.0 | 0.29.0 | **spec edit** `"0.24"`→`"0.29"` — Phase 3 |
| rand | 0.8.5 | 0.10.1 | **spec edit + code** — Phase 2 |
| socket2 | 0.5.10 | 0.6.3 | **spec edit (+code?)** — Phase 2 |
| toml | 0.8.23 | 1.1.2 | **spec edit (+code?)** — Phase 2 |
| rcgen | 0.13.2 | 0.14.8 | **spec edit + code** — Phase 2 |

Several targets already co-exist transitively (object 0.37.3, socket2 0.6.3, rand 0.10.1, http 1.4.0,
dashmap 6.1.0) → bumps partially consolidate the tree.

#### Phase-2 API-surface pre-scope (read-only)
- **rand**: `thread_rng()`→`rng()`, `gen_range`→`random_range`. Sites: lb-balancer
  {weighted_random,p2c,random}.rs, lb-io/pool.rs (test), lb/main.rs ×2 (jitter, ceil).
  `ring::rand` (quic_pool.rs) is RING, unaffected. Not crypto — LB selection + retry jitter.
- **socket2**: tiny — `SockRef`, `set_reuse_address/port`, `set_send_buffer_size`
  (lb-io/sockopts.rs, lb/main.rs:5013). No multicast v4/v6 methods → the v4/v6 renames likely
  don't touch us; verify 0.6 API at apply.
- **toml**: tiny — `toml::from_str`, `toml::Value`, `toml::de::Error` (lb-config, lb-controlplane).
  Stable across 1.x; verify.
- **rcgen**: pattern `CertificateParams::new → is_ca → extended_key_usages.push →
  KeyPair::generate() → params.self_signed(&key_pair) → cert.pem()/key_pair.serialize_pem()`
  + `generate_simple_self_signed(...)`. Sites: lb-quic {router,raw_proxy}.rs, lb/main.rs,
  lb-soak/config_gen.rs, lb-security {ticket×3,handshake}.rs. ALL test/dev/binary-e2e cert-gen
  (no production request path). 0.14 "signing-key API" verified at apply.

---

## PHASE 1 — routine patch group  →  **7 of 8 IN, 1 dropped**

dep-eng authored; verifier gates independently (below). Method: `cargo update --precise` per crate
(attributable, no transitive cascade); spec edits for 0.x-minor; **dual-version** retry for the two
that `--precise` couldn't move.

| crate | old | new | verdict |
|---|---|---|---|
| http | 1.4.0 | **1.4.1** | ✅ in |
| serde_json | 1.0.149 | **1.0.150** | ✅ in |
| libc | 0.2.184 | **0.2.186** | ✅ in |
| rustls | 0.23.38 | **0.23.40** | ✅ in |
| rustls-pki-types | 1.14.0 | **1.14.1** | ✅ in |
| dashmap | 6.1.0 | **6.2.1** | ✅ in |
| object | 0.36.7 | **0.37.3** | ✅ in (dual-version: lb-l4-xdp→0.37.3, aya keeps 0.36.7; read API stable, 0 code change) |
| prometheus | 0.13.4 | ~~0.14.0~~ | ❌ **DROPPED — carried** |

**prometheus 0.14 drop (genuine, guard-confirmed):** prometheus is shared with `foundations`, a
transitive of the **HELD** `tokio-quiche 0.19.0` (pulled via a version *range*, not a hard pin).
Taking prometheus 0.14 forces `foundations 4.5.0 → 5.7.1` → `tokio 1.51.1 → 1.52.3` +
tonic/opentelemetry/prost/governor of the held QUIC telemetry stack. No second consumer pins
foundations 4.5.0, so it can't fork the way object forks against aya's hard `^0.36`. Disturbing the
held surface is a blocker (R3) → **prometheus 0.14 carried forward** (CF: revisit when quiche/
tokio-quiche unhold, or pin foundations). object, by contrast, forks cleanly (aya's `^0.36` keeps
0.36.7 alive as the second consumer).

Code changes: **none** (object read API stable; loader.rs untouched). `cargo check --workspace
--all-features` exit 0. Held surface verified unchanged: quiche 0.29.1 (registry), tokio-quiche
0.19.0, foundations 4.5.0, tokio 1.51.1, aya 0.13.1. Commits: `97132c9f` (6) + `ed08ef7c` (object).

### Phase 1 binding gate (verifier, independent) — **GREEN** (commit ed08ef7c)
fmt clean · clippy `--all-targets --all-features -D warnings` clean · `cargo test --workspace
--all-features --no-fail-fast` ×3:

| run | passed | failed | ignored |
|---|---|---|---|
| 1 | 1512 | 0 | 18 |
| 2 | 1512 | 0 | 18 |
| 3 | 1511 | **1** | 18 |

The single run-3 failure = **CF-FCAP1-FLAKE** (known pre-existing, isolation-proven). Test
`h2h3_fcap1_over_cap_upload_never_complete`: a **vacuity** failure — the upload stalled on QUIC
flow-control at 67026399 B, **82465 B short** of the 67108864 B cap, so the over-cap Reset arm
wasn't reached (harness backpressure-masking, `fcap1-overcap-arm-backpressure-masked`). Confirmed
NOT a regression: passed runs 1&2; **isolation re-run 3/3 PASS** (forwarded 67075774 / 67067184 /
67108864 B — reaches the cap uncontended). None of the 7 bumps touch QUIC flow-control. No
assertion weakened (R2). Held surface intact (quiche 0.29.1, tokio-quiche 0.19.0, foundations
4.5.0, tokio 1.51.1). Logs: `audit/deps/s33-gate-phase1/`, `audit/deps/s33-fcap1-iso-{1,2,3}.log`.

> **Box constraint found (baked into the gate runner):** 15 GiB RAM / 0 swap → default ~8-way
> `--all-features` test compile OOMs (SIGKILL → cargo exit 101 / 0 tests — looks like a compile
> error). Fix: `CARGO_BUILD_JOBS=4`. The first gate attempt died this way; re-run with the cap was
> clean. Memory: `s33-box-15gb-ram-cap-cargo-jobs`.

**PHASE 1 COMPLETE** ✅ — 7 crates in, prometheus 0.14 carried.

## PHASE 2 — breaking-API group  →  **4 of 4 IN, 0 dropped**

dep-eng authored 4 attributable commits (one per crate, ordered socket2→toml→rand→rcgen), each with
a targeted `cargo test -p <affected>` confirm + held-surface guard. Lead reviewed the full source
diff: **purely mechanical, no smuggled logic** (R5). Held surface (quiche 0.29.1 / tokio-quiche
0.19.0 / foundations 4.5.0 / tokio 1.51.1 / aya 0.13.1) unchanged after every bump.

| crate | old | new | commit | adaptation |
|---|---|---|---|---|
| socket2 | 0.5.10 | **0.6.3** | `1028a550` | `set_nodelay`→`set_tcp_nodelay` ×3 (lb-io/sockopts.rs:97,150,206) — same TCP_NODELAY sockopt, renamed |
| toml | 0.8.23 | **1.1.2** | `2eaafc5d` | `.parse::<toml::Value>()`→`::<toml::Table>()` (lb-controlplane/lib.rs:234) — toml 1.0 `FromStr for Value` now parses a *bare value* not a *document*; `Table::from_str` restores document-parse intent |
| rand | 0.8.5 | **0.10.1** | `0b36fab5` | `thread_rng()`→`rng()`, `gen_range`→`random_range`, `use rand::Rng`→`{Rng, RngExt}` (random_range moved to RngExt ext-trait); 7 sites; `ring::rand` untouched; seeded-StdRng determinism intact |
| rcgen | 0.13.2 | **0.14.8** | `8a96940d` | `CertifiedKey.key_pair`→`signing_key` ×9 (struct now `CertifiedKey<S: SigningKey>`); 30 `KeyPair::generate()` locals + `self_signed(&key_pair)` unchanged; 6 Cargo.toml spec bumps |

**Staged-gating win:** the toml break compiled clean under `cargo check` but **panicked 7
lb-controlplane tests at runtime** — caught only because dep-eng ran `cargo test` per the Phase-1
lesson (plain check skips test targets). Two other changes (socket2 `set_tcp_nodelay`, rand
`RngExt`) were beyond the plan's prediction but pure mechanical renames the compiler pointed to.

Per-crate targeted tests all **0 failed** (lb-io, lb, lb-config, lb-controlplane, lb-balancer,
lb-security, lb-quic, lb-l7). Note: rcgen's lb-quic test needs `--features test-gauges` to compile
`grpc_h3_e2e` (pre-existing cfg-gate, rcgen-unrelated; the `--all-features` binding gate covers it).
Side effects (benign, attributable): toml 1.x pulled toml_parser/winnow1 + GC'd orphaned socket2
0.5.10; rcgen pulled x509-parser/asn1-rs ecosystem — no held-surface move.

### Phase 2 binding gate (lead-run, independent of author dep-eng) — **GREEN** (commit 4b68a539)
First gate attempt on `8a96940d` **FAILED** (clippy + test exit 101) — `error[E0609]: no field
key_pair on CertifiedKey` at **20 more sites in the root `tests/` directory** (`lb-integration-tests`
package). My Phase-2 plan grep covered `crates/*/tests` but not the root `tests/` dir, and dep-eng's
per-crate `cargo test -p` doesn't compile the root package — so the incomplete rename slipped to the
gate. **The full `--all-targets` gate caught exactly what per-crate testing structurally can't.**
Not a drop — completed the rename (fixup commit `4b68a539`, `cargo test --workspace --no-run` exit 0,
empty-proof: zero `.key_pair` field accesses remain). Lesson saved:
`dep-bump-compile-confirm-all-targets`.

Re-gate on `4b68a539`: fmt 0 · clippy 0 · test ×3 = **1512/0/18 / 1512/0/18 / 1512/0/18** (zero
failures, no fcap1 flake this run, no ENOSPC). The full suite — h2spec, WS matrix, gRPC-H3, all root
integration tests — green with all 4 adaptations. Held surface intact.

> Disk note: cumulative gate cruft filled eg-target to 44G → ENOSPC during the fixup; CF-DISK-1
> reclaim (drop `debug/incremental` + `debug/deps` executables, keep `.rlib`s) → 4.2G; re-gate built
> back to 28G/17G-free. Reclaim between phase gates. Memory: `s33-box-15gb-ram-cap-cargo-jobs`.

**PHASE 2 COMPLETE** ✅ — socket2 0.6.3, toml 1.1.2, rand 0.10.1, rcgen 0.14.8 all in, 0 dropped.

## PHASE 3 — H2 stack + WS library  →  **both increments IN**

| crate | old | new | commit | adaptation |
|---|---|---|---|---|
| hyper | 1.9.0 | **1.10.1** | `847a0a22` | caret update, **zero code change**; pulled h2 0.4.14 |
| h2 | 0.4.13 | **0.4.14** | (same) | — |
| tokio-tungstenite | 0.24.0 | **0.29.0** | `bffe34b3` | mechanical 0.29 API rewrap (below); +tungstenite 0.29; root spec only (members `workspace=true`) |

**hyper/h2:** lock delta = 2 version lines, no cascade; held surface intact. H2 conformance green
(h2spec generic + strict OK; h2_proxy_e2e 5/0; h2_validation_before_forward 3/0).

**tokio-tungstenite 0.29 — NOT dropped, adapted (lead-reviewed diff: pure API surface, R8 relay
logic untouched):**
1. `WebSocketConfig` now `#[non_exhaustive]` → struct-literal replaced by chaining setters
   (`.max_message_size`/`.max_frame_size`/`.max_write_buffer_size`) with **byte-identical values**
   from `default()` → the F-S27-2/R8 `max_write_buffer_size` bound is unchanged (ws_proxy.rs
   `tungstenite_config`, ws_r8_backpressure_plateau.rs).
2. `CloseFrame.reason` `Cow<str>` → `Utf8Bytes`: `Cow::Borrowed(lit)` → `Utf8Bytes::from_static(lit)`
   (same zero-copy static literals/codes; ws_proxy.rs ×4 + ws_proxy_e2e.rs).
3. `CloseFrame` owned (lost lifetime + `into_owned()`): `CloseFrame<'_>`→`CloseFrame`,
   `Some(f.into_owned())`→`Some(f)` (ws_proxy_e2e.rs).
4. `Message::Binary/Text/Ping` payload `Vec<u8>`/`String` → `Bytes`/`Utf8Bytes`: `.into()` at
   construction (loadgen.rs, **lb/main.rs:5021** prod WS broadcast — pure type wrap), `.to_vec()`/
   `.as_str()` on receive (`Bytes: PartialEq<Vec<u8>>` + `Utf8Bytes: Deref<str>` verified).

**Lead diff review:** the WS relay select-loop / backpressure detection / feed-flush logic is **not in
the diff** — only config/close-reason/payload-type wrapping changed. **R8 preserved** (proven by the
passing `ws_r8_backpressure_plateau` which depends on the exact migrated config values, + the 13.2s
`ws_h2_r8_backpressure`). dep-eng WS-test run: lb-l7/lb-quic 0-failed; root ws_h2_e2e 1/0, ws_h2_burst
1/0, ws_h2_conformance 4/0, ws_h2_upgrade_defer 3/0, ws_h2_r8_backpressure 1/0, ws_r8_backpressure_plateau
1/0, ws_proxy_e2e 7/0. Held surface intact after both increments.

### Phase 3 binding gate (lead-run) — _<pending gate on bffe34b3>_
### CF-S27-2 check (hyper 1.10.1 H2-upgrade backpressure) — **FINDING: UNCHANGED, still gated**
S30 found WS-over-H2 unviable: hyper's H2 CONNECT-upgrade write path sends **unconditionally on a
closed window** → unbounded h2 buffering (F-S27-2 DoS); WS-H2 stays gated. **Checked hyper 1.10.1
`proto/h2/upgrade.rs` (registry source) — the bug is STILL PRESENT, confirming S30's "identical in
1.10.1":**
- `UpgradedSendStreamTask::tick` (still named `tick`) does, on a zero window:
  `'capacity: loop { match poll_capacity(cx) { … Poll::Pending => break 'capacity } }` — the
  **`Pending` is swallowed by `break 'capacity`**, then control falls through to
  `me.rx.poll_next` → `me.h2_tx.send_data(SendBuf::Cursor(cursor), false)` **regardless of the
  closed window**. Exact S30 mechanism, same site.
- The 1.10.x `mpsc::channel(1)` bridge between `H2Upgraded::poll_write` and the task does **NOT**
  fix it: because the task never parks on the closed window, it keeps draining the cap-1 channel
  and feeding h2's send buffer, which buffers unbounded (`h2 share.rs`). The bridge bounds only the
  one in-flight chunk, not h2's window-less buffer.

⇒ **CF-S27-2 is NOT resolved by the hyper 1.9→1.10.1 bump.** WS-H2 correctly stays gated (no change
this session). *(Methodology note: my first read saw the `if capacity()==0 { poll_capacity }` gate
and wrongly inferred a fix — the bug is in how `Pending` is handled, not whether the gate exists;
reading the full `tick` body + heeding the s30 prior corrected it. feedback-symptom-not-attribution.)*
s30 memory re-confirmed (left as-is, already correct).

### Phase 3 binding gate (lead-run) — **GREEN** (gate on bffe34b3; fmt fixed → tip 2ea6c181)
clippy 0 · test ×3 = **1512/0/18 · 1512/0/18 · 1511/1/18** (run-3 fail = CF-FCAP1-FLAKE, the same
over-cap vacuity test, passed runs 1&2, Phase-1 isolation-confirmed 3/3 — known, not a regression).
No ENOSPC. **fmt initially FAILED** (dep-eng's `.into()`/`Utf8Bytes` edits pushed method chains past
100 chars; `--no-run` doesn't check fmt) → fixed with `cargo fmt --all` (whitespace-only, identical
tokens, commit `2ea6c181`, fmt clean at tip). The full suite (h2spec, WS matrix, gRPC-H3, root
integration tests) green with both bumps. Held surface intact.

### Phase 3 behavioral re-verify (lead-run targeted) — **GREEN** (both owner-relevant gates)
- **h2spec strict (crown jewel):** `h2spec_server_conformance_strict_passes` PASSED — the test runs
  the full `h2spec -S` suite and asserts exit 0 (0 failures); h2spec is a fixed conformance suite
  (the bump adds/removes no cases), so passing = the documented **146 passed / 1 skipped / 0 failed**
  holds under **hyper 1.10.1 / h2 0.4.14**. R11 crown-jewel intact, no regression. Log:
  `audit/deps/s33-h2spec-strict.log` (21985-byte h2spec stdout captured by the harness).
- **R13 WS close/reset burst:** `ws_proxy_e2e` (upgrade + relay + close + reset-mapping) run **×50 →
  50/50 PASS, 0 fail** → tokio-tungstenite 0.29 adaptation is stable, reset/close mapping intact, no
  flake introduced. WS matrix (H1/H2/H3) green in the binding gate.

**PHASE 3 COMPLETE** ✅ — hyper 1.10.1 + h2 0.4.14 + tokio-tungstenite 0.29 all in (0 dropped);
h2spec crown jewel + WS R8/R13 intact; CF-S27-2 unchanged (still gated).

---
### (original pre-scope, kept for reference)
- hyper 1.9→1.10.1 + h2 0.4.13→0.4.14: caret `cargo update` (no spec edit). Gate = **h2spec strict
  146/147** (`/home/ubuntu/.cargo/bin/h2spec` via `tests/h2spec_server_conformance.rs`) MUST hold
  (crown jewel) + H2 cells. CF-S27-2 check: note whether h2 0.4.14 poll_capacity / hyper 1.10.1
  changes the WS-H2 closed-window send_data picture (do NOT un-gate regardless).
- tokio-tungstenite 0.24→0.29 (**highest adaptation risk** — 5-ver jump). Surface we use:
  `WebSocketStream`(+`from_raw_socket`), `client_async`/`client_async_with_config`/`accept_async`,
  `Message::{Binary,Text,Close}`, `CloseFrame`, `protocol::frame::coding::CloseCode`,
  `WebSocketConfig`, `tungstenite::client::ClientRequestBuilder`, `handshake::derive_accept_key`,
  `protocol::Role`. Likely breaks across 0.24→0.29: `Message::Text`→`Utf8Bytes`,
  `Message::Binary`→`Bytes` (payload type change → `.into()` at construction/match),
  `CloseFrame` (owned, `reason: Utf8Bytes`), `WebSocketConfig` field/builder churn. Re-verify the
  WS matrix (H1/H2/H3) real-wire + R8 bound + upgrade/relay/close burst ≥50 + reset mapping (R13).
  WS regression → **drop tokio-tungstenite** (pin 0.24), keep the rest.

## PHASE 4 — full re-validation + promote
_<pending Phase 3>_

**Re-soak plan (lead pre-scope).** `scripts/soak/run-soak.sh <dur> <out> [sample] [scale]
[scenarios…]` (needs `eg-soak` + gateway release binaries built first). 12 scenarios:
| mission surface | scenario | expect |
|---|---|---|
| H1/H2 base | sc1_h1h1, sc1b_h1h2, sc2_h2h2 | BOUNDED |
| slowloris / 413 | sc3_slowloris, sc6_413teardown | BOUNDED |
| **Mode B** | sc4_modeb | BOUNDED |
| **Mode A** | sc5_modea | BOUNDED |
| **H3** | sc7_h3terminate | BOUNDED |
| **WS matrix** | sc8_ws_h1, sc8b_ws_h2, sc8c_ws_h3 | BOUNDED (R8) |
| **gRPC-H3** | sc9_grpc_h3 | **DRIFT — KNOWN sc9 carried baseline** (quiche collected leak; NOT a new finding) |

Datapath deps changed → soak is load-bearing: rustls/socket2 (all TLS+socket paths), hyper/h2
(H1/H2/WS-H2), tokio-tungstenite (WS, R8), rand (LB select). **Batch** the run (8-core can't
co-locate all 12 — S20/S21 lesson; over-saturation false-positives), ≥900s/scenario, sample 15s,
read verdict only from the completed run (R15). Every scenario BOUNDED **except sc9 DRIFT** = pass.

Other Phase-4 gates: ×3 (--no-fail-fast); standalone h2spec 146/147; WS matrix real-wire + R8/R13;
gRPC-H3 4-call-types+trailers (grpc_h3_e2e 16); F-MD-4 (h2h3_md_streaming_verify); clippy/fmt; cov
≥80%. PROMOTE per R11 if all green (sc9 DRIFT expected).

## VERDICT
_<pending>_
