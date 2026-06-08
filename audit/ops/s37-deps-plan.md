# S37 Workstream D — Dependency Upgrade Execution Plan

Research-only deliverable (no `cargo build/update/test/clippy/llvm-cov` run — disk/target
contention). Latest versions taken from the crates.io **sparse index** (`index.crates.io`,
the same source cargo reads — a plain GET, not a build). Breaking-change notes from each
crate's CHANGELOG / GitHub releases / docs.rs. Current resolved versions from `Cargo.lock`;
declared pins from root `Cargo.toml [workspace.dependencies]` (lines ~69–149) and
`crates/lb-l4-xdp/Cargo.toml:42` (object).

**Headline:** the tree is already almost entirely current. The *only* real source-graph
bumps are **tokio 1.51.1→1.52.3**, **socket2 0.6.3→0.6.4**, **prometheus 0.13.4→0.14.0**,
**object 0.37.3→0.39.1**, **aya 0.13.1→0.13.2**, **aya-obj 0.2.1→0.2.2**, and the held
cluster **foundations 4.5.0→5.7.2 + idna_adapter 1.1.0→1.2.2** (transitive, lockfile-only
pins — consciously un-hold). One dev-only **major**: **reqwest 0.12→0.13**. Everything else
(quiche, tokio-quiche, hyper, hyper-util, h2, rustls, http, bytes, serde, …) is already at
the latest release; those "bumps" are no-ops / relock-only.

---

## 1. Per-dep version table

`= latest` means the resolved Cargo.lock version already equals the newest stable release;
no edit needed (a `cargo update` may still pick up a patch-level relock).

| Crate | Declared pin | Resolved (lock) | LATEST (index) | Action | Risk |
|---|---|---|---|---|---|
| **tokio** | `1` (full) | 1.51.1 | **1.52.3** | relock to 1.52.3 | mechanical |
| io-uring | `0.7` | 0.7.12 | 0.7.12 | = latest | none |
| thiserror | `2` | 2.0.18 | 2.0.18 | = latest | none |
| anyhow | `1` | 1.0.102 | 1.0.102 | = latest | none |
| serde | `1` | 1.0.228 | 1.0.228 | = latest | none |
| serde_json | `1` | 1.0.150 | 1.0.150 | = latest | none |
| toml | `1` | 1.1.2+spec-1.1.0 | 1.1.2+spec-1.1.0 | = latest | none |
| bytes | `1` | 1.11.1 | 1.11.1 | = latest | none |
| **socket2** | `0.6` (all) | 0.6.3 | **0.6.4** | relock to 0.6.4 | mechanical |
| libc | `0.2` | 0.2.186 | 0.2.186 | = latest | none |
| tracing | `0.1` | 0.1.44 | 0.1.44 | = latest | none |
| tracing-subscriber | `0.3` | 0.3.23 | 0.3.23 | = latest | none |
| parking_lot | `0.12` | 0.12.5 | 0.12.5 | = latest | none |
| dashmap | `6` | 6.2.1 (+5.5.3 transit.) | 6.2.1 | = latest (leave 5.5.3) | none |
| http | `1` | 1.4.1 (+0.2.12 transit.) | 1.4.1 | = latest | none |
| **hyper** | `1` (full) | 1.10.1 | **1.10.1** | = latest (relock guard) | none |
| **hyper-util** | `0.1` (full) | 0.1.20 | 0.1.20 | = latest | none |
| http-body-util | `0.1` | 0.1.3 | 0.1.3 | = latest | none |
| **quiche** | `0.29` | 0.29.1 | **0.29.1** | = latest (MSRV 1.88, met) | none |
| **tokio-quiche** | `0.19` | 0.19.0 | **0.19.0** | = latest (MSRV 1.88, met) | none |
| rustls | `0.23` (ring,std,tls12,logging) | 0.23.40 | 0.23.40 | = latest | none |
| rustls-pki-types | `1` | 1.14.1 | 1.14.1 | = latest | none |
| tokio-rustls | `0.26` | 0.26.4 | 0.26.4 | = latest | none |
| rustls-pemfile | `2` | (2.x) | 2.x | = latest | none |
| tokio-util | `0.7` (rt) | 0.7.18 | 0.7.18 | = latest | none |
| **tokio-tungstenite** | `0.29` (handshake) | 0.29.0 | **0.29.0** | = latest (S33 already bumped) | none |
| futures-util | `0.3` | 0.3.32 | 0.3.32 | = latest | none |
| rand | `0.10` | 0.10.1 (+0.9/0.8 transit.) | 0.10.1 | = latest | none |
| **aya** | `0.13` | 0.13.1 | **0.13.2** | relock to 0.13.2 | mechanical |
| **aya-obj** | `0.2` | 0.2.1 | **0.2.2** | relock to 0.2.2 | mechanical |
| caps | `0.5` | 0.5.x | 0.5.x | = latest | none |
| libfuzzer-sys | `0.4` | 0.4.x | 0.4.x | = latest | none |
| **prometheus** | `0.13` (no-default, process) | 0.13.4 | **0.14.0** | bump 0.13→0.14 | mechanical (verify labels) |
| proptest | `1` (no-default,std,fork,timeout) | 1.11.0 | 1.11.0 | = latest | none |
| loom | `0.7` | 0.7.x | 0.7.x | = latest | none |
| **object** (lb-l4-xdp:42) | `0.37` (no-default,read,std) | 0.37.3 (+0.36.7 transit.) | **0.39.1** | bump 0.37→0.39 | mechanical |
| **reqwest** (dev) | `0.12` (no-default,rustls-tls,http2) | 0.12.28 | **0.13.4** | bump 0.12→0.13 (dev-only **major**) | likely-adaptation (low blast) |
| rcgen (dev) | `0.14` (pem) | 0.14.8 | 0.14.8 | = latest | none |
| ring (dev) | `0.17` | 0.17.14 | 0.17.14 | = latest | none |
| h2 (dev) | `0.4` | 0.4.14 | 0.4.14 | = latest | none |
| **foundations** (transitive, lock pin) | — | 4.5.0 | **5.7.2** | un-hold → 5.7.2 (MSRV n/a; un-pin Cargo.lock) | likely-adaptation (transitive) |
| **idna_adapter** (transitive, lock pin) | — | 1.1.0 | **1.2.2** | un-hold → 1.2.2 (MSRV 1.86, met) | mechanical |

Notes on dual lockfile versions (leave the transitive copy unless it drops out naturally on
relock): `dashmap 5.5.3` (transitive, hashbrown 0.14), `http 0.2.12` (transitive),
`thiserror 1.0.69` (transitive), `rand 0.8.5 / 0.9.4` (transitive), `object 0.36.7`
(transitive via the gimli/backtrace family — distinct from our direct 0.37→0.39 line).

---

## 2. Breaking-change notes (non-trivial bumps)

### tokio 1.51.1 → 1.52.3  (PR #224 set) — MECHANICAL
- 1.52.0: additive (`AioSource::register_borrowed`, `unix::pipe::try_io`, io_uring
  `AsyncRead` for `File`, unstable builder knobs). No removals on our surface.
- 1.52.1/.2: **reverts** of perf/regression changes (spawn_blocking hang revert; LIFO
  slot-stealing revert from 1.51.0). 1.52.3: mpsc `len()` underflow fix, `OwnedPermit::release`
  notify, `RwLock` `max_readers != 0` requirement, `try_recv()` returns `Empty` when closed
  with outstanding permits. None of these change a public signature we call.
- **Risk: mechanical.** We use `tokio = { version = "1", features = ["full"] }`; caret already
  admits 1.52.x. Expect lock-line-only delta.

### socket2 0.6.3 → 0.6.4 (PR #224 set) — MECHANICAL
- `impl Send for MsgHdr(Mut)`; horizonOS/n3ds + QNX tweaks; Windows `only_v6` bool-width fix.
  No API change on Linux. Patch bump.

### prometheus 0.13.4 → 0.14.0 (PR #224 set) — MECHANICAL (verify label sites)
- **API change #537:** label-value setters now take `AsRef<str>` for owned label values
  (instead of `&str`). This is **additive/widening** — existing `&[&str]` call sites still
  compile (`&str: AsRef<str>`). Our usage is `with_label_values(&[listener])` /
  `with_label_values(&["200"])` (lb-observability/src/lib.rs:394,405,561-565) — unaffected.
- `Encoder`/`TextEncoder` unchanged: `encode<W: Write>(&self, &[MetricFamily], &mut W) -> Result<()>`
  (verified docs.rs 0.14.0) — our `encoder.encode(&mfs, &mut buf)`
  (prometheus_exposition.rs:29) compiles as-is. `Registry::gather()` unchanged.
- **`process` feature still exists** in 0.14.0 (`["libc","procfs"]`, verified index features
  blob). Our `default-features = false, features = ["process"]` stays valid. 0.14 bumps the
  transitive `procfs 0.16→0.17` and (under the disabled `protobuf` default) `protobuf→3.7.2`
  for RUSTSEC-2024-0437 — we keep default-features off so protobuf is not pulled.
- MSRV 0.14 = 1.81 (we are 1.88). Our metric *types* (IntCounter/IntGauge/IntCounterVec/
  IntGaugeVec/Histogram/Registry/Collector) are all retained.
- **Risk: mechanical.** Expect ZERO source change; the AsRef widening is the only API note and
  it does not break `&[&str]`. CONFIRM at gate by reading "Finished" on the workspace compile.

### object 0.37.3 → 0.39.1 (PR #224 set) — MECHANICAL
- Our entire usage is read-only ELF: `object::File::parse(elf)` →
  `.section_by_name("license")` → `.data()` via traits `Object` + `ObjectSection`
  (lb-l4-xdp/src/loader.rs:34,732,734,744). (The `Object::parse` at loader.rs:1033 is
  **aya_obj's** `Object`, a name-clash — see the comment at :32-34 — not the `object` crate.)
- 0.38.0 breaking: `macho::EXPORT_SYMBOL_FLAGS` → u8; `read::elf::Dyn::string` StringTable
  type fix. **Neither touched by us.** (0.38 "all-features MSRV" → 1.87; we are 1.88.)
- 0.39.0 breaking: `read::NativeFile` → `NativeEndian` (we use `File`, not `NativeFile`);
  `elf::Dyn32/64::d_tag` signed + `DT_*` → i64 + `write::elf::Writer::write_dynamic`
  (we read no dynamic tags, no writer); `unaligned` feature is now a no-op (we don't set it);
  `read::SymbolMapName::new` gains `size` (we don't construct it). 0.39.1: PE imports +
  Mach-O additive. **None on our path.** `read`+`std` features retained.
- **Risk: mechanical.** `File::parse` / `section_by_name` / `data()` / the `Object` +
  `ObjectSection` trait names are all unchanged. Expect ZERO source change.

### aya 0.13.1 → 0.13.2 / aya-obj 0.2.1 → 0.2.2 — MECHANICAL
- Patch bumps within 0.13.x / 0.2.x. The committed XDP ELF uses aya legacy maps (only the
  `aya` loader, not bpftool/ip, loads it — see prior session facts). No public-API churn at
  this patch level expected; relock + the existing XDP load smoke is the proof.

### foundations 4.5.0 → 5.7.2 (transitive, lockfile-pinned — un-hold) — LIKELY-ADAPTATION
- foundations is a **transitive** dep (no entry in any `crates/*/Cargo.toml`; pulled via
  tokio-quiche). It is pinned ONLY in Cargo.lock (no `[patch]`), per the MSRV-pin note at
  root `Cargo.toml:52-59`: held for **isolation** from prior quiche-only sessions, NOT for
  MSRV (1.88 already clears darling 0.23 = 1.88 / icu 2.2 = 1.86).
- 5.x is a **major** jump (4.5→5.7). Because it is transitive and read through tokio-quiche,
  any breakage would surface as a *resolution/feature* conflict, not a source-API break in
  our code. RISK is that tokio-quiche 0.19's own `foundations` requirement may cap the
  resolvable version (if tokio-quiche 0.19 requires `foundations ^4`, the lock CANNOT move to
  5.x without a tokio-quiche bump — and tokio-quiche 0.19 is already latest). **Verify the
  tokio-quiche→foundations requirement BEFORE attempting the un-hold** (`cargo tree -i
  foundations` after the relock; if 0.19 pins ^4, foundations stays at 4.5.0 and that is
  CORRECT, not a failure — record it as a held-by-upstream finding).
- **RUSTSEC interaction:** `deny.toml` + `.cargo/audit.toml` waive `RUSTSEC-2024-0320`
  (yaml-rust 0.4 unmaintained) attributing it to **foundations 4.5 → serde_yaml 0.8 →
  yaml-rust**. If foundations DOES move to 5.x and drops serde_yaml/yaml-rust, that waiver
  becomes stale → prune it (and re-run audit to confirm it no longer fires). If foundations
  stays at 4.5 (upstream cap), the waiver stays.
- **Risk: likely-adaptation OR held-by-upstream.** Treat the un-hold as best-effort: attempt
  it, and if tokio-quiche caps it, leave foundations at 4.5.0 with an updated isolation note.

### idna_adapter 1.1.0 → 1.2.2 (transitive, lockfile-pinned — un-hold) — MECHANICAL
- Transitive (idna/url family). MSRV 1.2.2 = 1.86 (we are 1.88). Patch/minor; un-pin in
  Cargo.lock and relock. **Risk: mechanical.**

### reqwest 0.12.28 → 0.13.4 (**dev-only major**) — LIKELY-ADAPTATION (low blast)
- reqwest is a **dev-dependency only** — used in exactly two test files
  (`tests/h2h1_md_streaming_verify.rs`, `tests/h2_proxy_e2e.rs`). It NEVER enters the product
  binary, so this major has no production blast radius.
- 0.13 breaking changes (seanmonstar blog + CHANGELOG): (a) **default TLS is now rustls**
  (was native-tls) and the rustls crypto provider default is **aws-lc** (was ring);
  (b) `query`/`form` are now opt-in features (off by default → builds without serde possible);
  (c) deprecated methods removed (e.g. trust-dns → hickory); (d) several TLS methods renamed
  with soft-deprecation aliases (`use_rustls_tls()` → `tls_backend_rustls()`).
- Our dev pin is `default-features = false, features = ["rustls-tls", "http2"]` — we already
  opt into rustls-tls explicitly, so the default-TLS flip does not change our wiring. **Watch
  for:** (i) the aws-lc-vs-ring crypto-provider default could pull `aws-lc-rs` into the dev
  graph (we standardise on `ring` elsewhere — if a duplicate crypto provider or a
  "no process-default CryptoProvider" panic appears in those two tests, pin reqwest's rustls
  to ring via its feature set or install a process default); (ii) if either test calls a
  renamed TLS method or relies on `query()`/`form()` without the feature, add the feature or
  rename. **Risk: likely-adaptation, dev-only.** If 0.13 proves fiddly, it is acceptable to
  HOLD reqwest at 0.12.28 (latest 0.12) — it is a test harness, not shipped surface.

---

## 3. hyper #4050 VERDICT (WS-over-H2 backpressure)  ← **CRITICAL**

**VERDICT: NOT FIXED in any released hyper. WS-H2 STAYS GATED. No fork.**

Evidence (primary sources, fetched this session):

1. **The bug.** hyper#4049 (open) + its fix PR **#4050** (open) describe
   `UpgradedSendStreamTask::tick()` in `src/proto/h2/upgrade.rs`: when
   `poll_capacity()` returns `Poll::Pending`, the code does `break 'capacity` and **falls
   through to `send_data()`**, pushing into the h2 send buffer with zero flow-control
   capacity → unbounded gateway memory for a non-reading WS-H2 client. Introduced in hyper
   **v1.8.0** (PR #3967, the unsafe-transmute removal refactor); the issue names v1.8.0,
   1.8.1, 1.9.0 as affected.

2. **It is STILL present in the latest release (v1.10.1).** Fetched
   `raw.githubusercontent.com/hyperium/hyper/v1.10.1/src/proto/h2/upgrade.rs` — the
   `'capacity` loop still ends with `Poll::Pending => break 'capacity,` (verbatim), i.e. the
   buggy fall-through path is unchanged at the latest tag.

3. **No release contains a fix.** Latest hyper release = **v1.10.1** (2026-05-29, the
   newest tag). The v1.10.1 / v1.10.0 / v1.9.0 CHANGELOG entries contain **no** upgrade /
   CONNECT / backpressure / flow-control / poll_capacity / send-buffer fix (verified
   against the v1.10.1 CHANGELOG.md). A repo search for merged PRs mentioning
   `poll_capacity` in the upgrade-backpressure context returned **zero** results.

4. **The fix is unmerged.** PR #4050 is `state: open`, `merged: false`, base `master`,
   `mergeable_state: behind` — not in `master`, not in any tag.

**Decision (per the rule lead set):** un-gate WS-H2 ONLY IF the fix is in the adopted
release. The adopted hyper release is 1.10.1 (already current) and it does **not** contain
the fix. Therefore:
- **WS-H2 stays gated.** Leave `h2_extended_connect` default `false`
  (`crates/lb-config/src/lib.rs:766`) and the SETTINGS/intercept guard at
  `crates/lb-l7/src/h2_proxy.rs:837-842` untouched.
- **Leave the H2 plateau case `#[ignore]`d** at `tests/ws_r8_backpressure_plateau.rs:380`
  (`fn h2_backend_flood_plateaus_against_nonreading_client`). Do NOT un-`#[ignore]` it.
- **No hyper fork.** Re-confirm CF-S27-2 stays OPEN, attributed upstream (hyper#4049/#4050),
  and add the verdict (1.10.1 still buggy, #4050 unmerged) to the audit trail.
- **Re-check trigger for a future session:** when a hyper release lands that merges #4050
  (watch the CHANGELOG for an upgrade/CONNECT backpressure / poll_capacity fix), adopt it,
  then PROVE the un-gate by removing `#[ignore]` on
  `tests/ws_r8_backpressure_plateau.rs:~380` and showing the producer plateaus (n <
  `PLATEAU_CEILING`) before flipping the config default.

---

## 4. Staged execution order (S33-style — each stage gate-green, lock-delta-bounded)

Box reality (memory): ~15 GiB RAM / shared CARGO_TARGET_DIR → cap `CARGO_BUILD_JOBS=4` or the
`--all-features` test compile OOMs (looks like a compile fail, exit 101 / 0 tests). Compile-
confirm with `cargo test --workspace --all-features --no-run` (NOT `check` — skips tests;
NOT per-crate `-p` — skips the root `lb-integration-tests` pkg). Caret-update per stage
(`cargo update -p <crate> --precise <ver>`), keep the lock delta auditable. Commit per stage
with explicit paths (no `-A`, no stash; NO Claude/AI attribution). Do NOT promote on a single
green run for any flake-prone target.

**Pre-flight (no build):** supersede the two open Dependabot PRs by folding their content
here, then close them on GitHub referencing this branch.
- **PR #224** (`dependabot/cargo/dependencies-197b239d55`) — exact set confirmed via
  `gh api`: **tokio 1.51.1→1.52.3, socket2 0.6.3→0.6.4, prometheus 0.13.4→0.14.0,
  object 0.37.3→0.39.1** (files: Cargo.lock, Cargo.toml, crates/lb-l4-xdp/Cargo.toml).
  Folded into Stage 1 (tokio/socket2) + Stage 2 (prometheus/object).
- **PR #226** (`dependabot/github_actions/actions-ffa548d3e0`) — GitHub Actions YAML bumps
  (files: ci.yml, prod-readiness-gates.yml, release.yml, scheduled-scans.yml):
  checkout 4→6, upload-artifact 4→7, download-artifact 4→8, setup-buildx 3→4, login 3→4,
  metadata 5→6, build-push 6→7, gh-release 2→3. Current pins in our workflows verified as
  v4/v4/v4/v3/v3/v5/v6/v2 respectively. Apply in Stage 6.

**Stage 1 — patch group (low-risk relocks).**
`tokio→1.52.3`, `socket2→0.6.4`, plus a caret relock that naturally picks up any pending
patch on serde/serde_json/libc/rustls/http/bytes/thiserror/anyhow/tracing*/parking_lot/
io-uring/rcgen/ring/tokio-util/tokio-rustls/rustls-pki-types/http-body-util/hyper/hyper-util/
h2 (all already at latest → expect 0 or near-0 additional lines). No `Cargo.toml` edit needed
for tokio/socket2 (caret `1` / `0.6` already admit the targets) — `--precise` the lock.
Gate: `cargo test --workspace --all-features --no-run` (CARGO_BUILD_JOBS=4) → Finished;
clippy `-D warnings` (full gate cmd, `--all-targets --features test-gauges`); fmt.

**Stage 2 — prometheus 0.14 + object 0.39 (folds PR #224's two non-patch members).**
Edit root `Cargo.toml:139` `prometheus = "0.13"` → `"0.14"` (keep `default-features = false,
features = ["process"]`); edit `crates/lb-l4-xdp/Cargo.toml:42` `object = "0.37"` → `"0.39"`
(keep `default-features = false, features = ["read","std"]`). Expect ZERO source change
(§2 analysis). Gate: workspace `--no-run`; clippy; fmt; **plus** run the lb-l4-xdp XDP load
smoke (the existing aya-legacy-map load test) to prove object/aya read path intact; **plus**
exercise the `/metrics` exposition test + a `with_label_values` metric to prove prometheus
0.14 label/encoder path. Also bump `aya→0.13.2`, `aya-obj→0.2.2` here (same XDP smoke covers
them). If prometheus or object forces a code change, STOP and record it (would contradict the
mechanical prediction).

**Stage 3 — quiche / tokio-quiche (NO-OP — already latest).**
quiche 0.29.1 and tokio-quiche 0.19.0 are already the newest releases (MSRV 1.88, met). No
edit. Still run the **h3spec gate** (`scripts/ci/h3spec-check.sh`, the 12 named CF-QUICHE-
UPGRADE waivers) post-relock to prove no h3 conformance regression slipped in from the
transitive churn. If foundations is being un-held (Stage 5), the quiche/h3 path is the most
likely place a transitive break would surface — keep h3spec in this stage's gate.

**Stage 4 — hyper / hyper-util / h2 (NO-OP — already latest) + apply the #4050 decision.**
hyper 1.10.1 / hyper-util 0.1.20 / h2 0.4.14 are already newest. No edit. **h2spec must stay
147/147 strict** — re-run the D-4 conformance path
(`prod-readiness-gates.yml:195` → `h2spec -t -k -h 127.0.0.1 -p 8443 --strict`, the gateway
"passes 147/147") to prove the relock didn't disturb H2. **Apply §3:** WS-H2 stays gated;
leave the `#[ignore]` on `ws_r8_backpressure_plateau.rs:380`; CF-S27-2 stays open; no fork.

**Stage 5 — held cluster un-hold (foundations + idna_adapter).**
Attempt `idna_adapter→1.2.2` (mechanical, MSRV 1.86) and `foundations→5.7.2`. FIRST run
`cargo tree -i foundations` after relock to see what requires it: if tokio-quiche 0.19 caps
it at `^4`, foundations CANNOT move to 5.x without a tokio-quiche bump (already latest) →
leave it at 4.5.0 and update the isolation note in root `Cargo.toml:52-59` to say
"held-by-upstream (tokio-quiche 0.19 requires foundations ^4)", which is a finding, not a
failure. If foundations DOES move to 5.x and drops serde_yaml/yaml-rust, **prune
`RUSTSEC-2024-0320`** from `deny.toml` + `.cargo/audit.toml` and re-run audit to confirm it
no longer fires. Gate: workspace `--no-run`; cargo-deny + cargo-audit; the soak/h3 paths.

**Stage 6 — CI actions (supersede PR #226) + dev reqwest (optional) + MSRV prose.**
- Bump the 8 GitHub Actions in `.github/workflows/*.yml` to PR #226's targets
  (checkout v6, upload-artifact v7, download-artifact v8, setup-buildx v4, login v4,
  metadata v6, build-push v7, gh-release v3). YAML-only; verified on push (no local cargo).
- **reqwest 0.12→0.13** (dev-only major): edit root `Cargo.toml:184`
  `reqwest = { version = "0.13", default-features = false, features = ["rustls-tls","http2"] }`.
  Gate: compile + run the two consumer tests (`h2h1_md_streaming_verify.rs`,
  `h2_proxy_e2e.rs`). Watch for the aws-lc-vs-ring crypto-provider default (§2). If it
  resists, HOLD reqwest at 0.12.28 (test harness, not shipped) and note it.
- **MSRV prose fix (no version change):** correct the stale "1.85" justifications now that
  MSRV is 1.88 — `deny.toml:28-38` ("pinned to keep workspace MSRV at 1.85") and
  `.cargo/audit.toml:21-24,31` ("MSRV 1.85 blocks the fix" / "Revisit when MSRV bumps to
  1.88" — MSRV already IS 1.88, so re-evaluate `RUSTSEC-2026-0009` time-DoS: if 1.88 admits
  time ≥0.3.47, that waiver may be droppable — verify, don't assume). `rust-toolchain.toml`
  already correct at 1.88. Bump MSRV further ONLY if an adopted crate hard-requires it
  (none of the above does — highest is object-all-features 1.87, prometheus 1.81; all < 1.88).

**Final (Phase-4, lead-coordinated):** unified re-validation = ×3 (`--no-fail-fast`),
h2spec strict 147/147, h3spec (12 waivers), WS matrix (H1 green; H2 stays gated/ignored),
gRPC (H1/H2/H3), R8 backpressure, and re-soak (sc9 BOUNDED ~22 MB @ cap 1000 from S36-A;
watch for any drift from the dep churn). Then promote `--no-ff`. Do NOT promote on one green
run for flake-prone targets (fcap1 etc.).

---

## 5. Per-crate risk flag (summary)

**Mechanical (relock or 1-line pin, zero source change expected):**
tokio 1.52.3, socket2 0.6.4, prometheus 0.14.0, object 0.39.1, aya 0.13.2, aya-obj 0.2.2,
idna_adapter 1.2.2, and every `= latest` no-op (hyper, hyper-util, h2, quiche, tokio-quiche,
rustls, tokio-rustls, rustls-pki-types, http, bytes, serde, serde_json, toml, thiserror,
anyhow, tracing, tracing-subscriber, parking_lot, dashmap, tokio-util, futures-util, rand,
io-uring, http-body-util, rcgen, ring, proptest). CI actions (PR #226) — YAML-only.

**Likely-adaptation:**
- reqwest 0.12→0.13 (dev-only **major**; rustls-default + aws-lc crypto provider + opt-in
  query/form + renamed TLS methods; low blast — two test files; HOLD-at-0.12 acceptable).
- foundations 4.5→5.7 (transitive **major**; un-hold is best-effort — may be capped by
  tokio-quiche 0.19 at ^4, in which case leave at 4.5.0 as held-by-upstream).

**Likely-drop / hold (conscious):**
- foundations may stay 4.5.0 if tokio-quiche 0.19 requires `^4` (verify with `cargo tree -i`;
  not a failure).
- reqwest may stay 0.12.28 if 0.13's crypto-provider default fights our ring standard (dev
  harness only).
- The transitive duplicate lines (dashmap 5.5.3, http 0.2.12, thiserror 1.0.69, rand 0.8/0.9,
  object 0.36.7) are owned by OTHER crates' requirements — leave them; do not force-dedupe.

**No bump (already latest):** quiche, tokio-quiche, hyper, hyper-util, h2, tokio-tungstenite
(S33 already took it to 0.29), rustls, and all the patch-current crates above.
