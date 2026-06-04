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
- test `--workspace --all-features --no-fail-fast` ×3: _<filled when complete>_
  (S31 reference on same commit: 1512/0/18.)

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

## PHASE 1 — routine patch group
_<pending>_

## PHASE 2 — breaking-API group
_<pending>_

## PHASE 3 — H2 stack + WS library
_<pending>_

## PHASE 4 — full re-validation + promote
_<pending>_

## VERDICT
_<pending>_
