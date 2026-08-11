# S45A — independent verification (author ≠ verifier)

**Reviewer:** `verifier` (senior Rust review role)
**Branch:** `feature/de-slop-s45a` @ `90a0dc97` · **Baseline:** `main` @ `ff39fa08` · **PR:** #258
**Scope reviewed:** the whole sweep — 254 changed `.rs` files, 24 changed shell/CI/workflow files,
20 deleted root scripts, 8 archived scripts.

---

## VERDICT

**GO — conditional on two one-line comment restorations (F-1, F-2) and one mechanical re-wrap (F-3).**

Nothing found is a correctness, safety, CI or behaviour risk. Code identity is proven, every gate is
green and non-vacuous, no test was removed, no `unsafe` block lost its SAFETY note, and no deleted or
moved script is referenced anywhere. The sweep also *corrected* a large class of pre-existing
misinformation (below), which is a real quality gain, not just deletion.

The conditions are cheap (two comments, one `fmt`-style pass) and all three are things the sweep's
own standard binds it to. I would not promote without F-1; F-2/F-3 are the lead's call.

| category | result |
|---|---|
| Behaviour change | **NONE** — proven, all 5 differing files verified benign by hand |
| Tests removed | **NONE** — 1582 tests / 308 files, per-file identical |
| `unsafe` / SAFETY | **NO REGRESSION** — 88 blocks both sides; SAFETY coverage improved by 1 |
| Gate-read strings | **ALL 10 PRESENT** verbatim |
| Deleted/moved scripts | **ZERO dangling references** |
| CI scripts / workflows | **structurally identical**; all documented rationale retained |
| `cargo fmt --all --check` | **PASS** |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **PASS** (non-vacuous) |
| `scripts/ci/doc-lint.sh` | **PASS** |
| Knowledge regression | **4 LOST of 150 sampled** (2 material, 2 self-evident) |
| Readability | content **excellent**; physical formatting **regressed** in 3 crates |

---

## FINDINGS

### F-1 · SHOULD-FIX (blocking for promote) — the S37-C / R6 SIGNAL-LOSS rationale is entirely gone

**Location:** `crates/lb/src/main.rs:2184-2185` (install site), `crates/lb/src/main.rs:2397-2431`
(`struct LifecycleSignals` / `install()`)

`main` carried, immediately above the install:

```
// SIGNAL-LOSS FIX (S37-C, R6): install the lifecycle-signal streams
// ONCE here and reuse them across every loop iteration. A SIGTERM/
// SIGINT landing while we service a non-terminal SIGHUP/SIGUSR1 is
// then never lost (it latches on the persistent stream). On non-unix,
// only Ctrl-C is wired (Windows operators drain via Ctrl-C).
```

The branch has **no comment at all** there:

```rust
    #[cfg(unix)]
    let mut lifecycle_signals = LifecycleSignals::install()?;
    let signal_kind = loop {
```

and `struct LifecycleSignals` / `install()` / `recv()` carry no doc either. The fact is not
recoverable from the code: the install sits outside the loop, which reads as an ordinary hoist. A
future editor moving it inside the loop (or reinstalling per iteration to "reset" the streams) silently
re-opens a **measured** SIGTERM-loss bug. This is exactly a clause-2 catch ("if you change this, X
breaks") and `s45a-inv-core.md` names it as a KEEP:

> `KEEP | crates/lb/src/main.rs:2982-2986 + 3292-3304 | SIGNAL-LOSS FIX (S37-C, R6): why the
> lifecycle-signal streams are installed ONCE outside the loop … Canonical "use X not Y because Y
> does Z".`

The sibling non-terminal-signal rationale (SIGHUP/SIGUSR1 are serviced-and-`continue`d so an operator
can re-signal after a rejected push) is gone with it.

**Suggested fix** — one line above `main.rs:2184`:

```rust
    // S37-C/R6: installed ONCE outside the loop. Re-installing per iteration LOSES a SIGTERM that
    // lands while a non-terminal SIGHUP/SIGUSR1 is being serviced; a persistent stream latches it.
```

---

### F-2 · SHOULD-FIX — RFC 6455 §7.1.5 citation + the `ws_handle_client_fin` mechanism dropped

**Location:** `crates/lb/src/main.rs:4716`

Now:

```rust
    /// **Client stream-FIN (no WS Close frame) → abnormal close.** The client closes its WS send half by FINning the H3 tunnel stream WITHOUT a WS Close frame.
```

What was cut is the half that says *what the test proves*: that `conn_actor::ws_handle_client_fin`
maps the FIN to a **clean EOF, NOT a Reset**, and that per RFC 6455 §7.1.5 the only clean close is the
Close-frame handshake. The standard is explicit here — *"RFC citations that justify a behavior a
reader would otherwise 'fix' — keep the citation, drop the recitation."* This dropped the citation and
kept the setup narration, which is the wrong half. The fact survives only inside the assert string at
`:4745`, so the test still fails loudly; but a reader of `ws_handle_client_fin` has no signal that
switching FIN→Reset there is the thing this test guards.

Same line is also 172 columns (see F-3).

**Suggested fix:** append to the existing line — `` `ws_handle_client_fin` must map the FIN to a clean
EOF, NOT a Reset: per RFC 6455 §7.1.5 the gateway must not fabricate a clean Close from a bare FIN. ``

---

### F-3 · SHOULD-FIX — 616 comment lines exceed 100 columns (main: 16). Formatting betrays the tool.

Multi-line blocks were collapsed onto one physical line without re-wrapping. The **content** is good;
the **shape** is not something a human would type.

| crate | >100-col comment lines | longest |
|---|---:|---:|
| `lb-l4-xdp` | 337 | 395 cols |
| `lb-soak` | 101 | 336 cols |
| `lb` | 97 | 324 cols |
| `lb-config` | 30 | 106 |
| `lb-io` | 21 | 107 |
| `lb-observability` | 14 | 105 |
| `lb-security` | 11 | 110 |
| `lb-core` / `lb-balancer` | 5 | 103 |
| **total** | **616** (441 >120 cols, 87 >200 cols) | **395** |
| *main baseline* | *16* | *131* |

`lb-quic` and `lb-l7` are **clean** — their round-2 sweepers re-wrapped (`a08c6f4a` "re-wrap long
comment lines to <=100; cargo fmt -p lb-quic"). So this is a consistency gap between sweepers, with an
in-session precedent for the fix.

Worst offenders: `crates/lb-l4-xdp/tests/round8_conntrack_state.rs:3` (395), `crates/lb-l4-xdp/tests/pod_padding.rs:3`
(365), `crates/lb-l4-xdp/tests/xdp_attach_mode.rs:48` (358), `crates/lb-l4-xdp/src/stats_export.rs:159`
(346), `crates/lb-soak/src/loadgen.rs:885` (336), `crates/lb/src/main.rs:456` (324).

Not a CI risk — there is no `rustfmt.toml`, and stock rustfmt does not wrap comments (`wrap_comments`
is nightly-only), so `cargo fmt --check` stays green either way. It is a risk to the *stated goal* of
the session.

**Suggested fix:** re-wrap to ≤100 in `lb-l4-xdp`, `lb-soak`, `lb` only (535 of the 616).

---

### F-4 · NOTE — two `// SAFETY:` invariant labels dropped where the guard is adjacent

- `crates/lb-l7/src/h2_proxy.rs:1109-1110` lost `// SAFETY: guarded by \`is_data()\`.`
  The guard is literally the enclosing `if frame.is_data() {`, and `unwrap_or_default()` is not a
  panic site, so the code reads fine. **Defensible deletion.**
- `crates/lb-grpc/src/frame.rs:33` lost `// SAFETY of .get(): we checked len >= GRPC_HEADER_SIZE above.`
  Every access in that function is `.get(..).ok_or(Incomplete)?` — panic-free by construction.
  **Defensible deletion.**

Neither annotates an `unsafe` block, so hard-constraint #3 is not violated. Reporting them only
because the invariant census flags them; I do **not** recommend restoring either.

The census's other three SAFETY-label losses are false alarms — the label was dropped but the fact
compressed and kept: `lb-core/src/shutdown.rs:112-113`, `lb-io/src/idle_send.rs:44`,
`lb-l7/src/h2_proxy.rs:2363`.

---

### F-5 · NOTE — one `quiche-0.28` behavioural claim left on a 0.29.1 tree

`crates/lb-quic/tests/h3_h3_stream_e2e.rs:2389` — `"quiche-0.28 §7.1 gap (no content-length)…"`.
The sweep cleaned 12 of the 13 stale `quiche 0.28` mentions; this one survives because it is a
*version-specific behavioural claim*, which `s45a-inv-lb-quic.md` §D correctly said must be re-checked
rather than bulk-rewritten. Leaving it is the right call; flagging so it is not mistaken for an
oversight. Non-blocking.

---

### F-6 · NOTE — `scripts/perf/` is now an empty directory

All five `s39-*.sh` moved to `scripts/archive/`. Git does not track empty directories, so it simply
vanishes on a fresh clone. Nothing references `scripts/perf` (verified by anchored grep across all
tracked files). Non-blocking; mentioned only so it is a known consequence.

---

### Categories with NOTHING to report

State explicitly, as requested:

- **Behaviour changes beyond the five authorized files:** none.
- **Removed or weakened tests:** none.
- **Lost `unsafe` SAFETY comments:** none.
- **Broken gate-read strings:** none.
- **Dangling references to deleted / moved scripts:** none.
- **Changed CI logic, waiver lists, coverage patterns, doc-lint file sets:** none.
- **Lost `#[allow(...)]` justifications:** none (see the table below — 9 explicitly-justified allows
  on both sides; the single attribute delta is the authorized `_gauge_vec_anchor` removal).
- **`.md` / `.toml` / Cargo dependency-provenance comments:** untouched — zero non-`.rs`, non-`.sh`,
  non-workflow files changed.

---

## CHECK 2 — CODE IDENTITY

```
$ python3 audit/craft/s45a-code-identity.py main
S45A code-identity proof — 254 .rs files changed vs main
  5 file(s) differ: TOKENS DIFFER = real code change; REFLOW ONLY = rustfmt layout, behaviour-neutral
    TOKENS DIFFER  crates/lb-observability/src/xdp_metrics.rs
    TOKENS DIFFER  crates/lb-quic/src/h3_bridge.rs
    TOKENS DIFFER  crates/lb-quic/tests/grpc_h3_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h3_stream_e2e.rs
```

Exactly five, as claimed. **No sixth file.** I audited the tool itself first (string / char / raw-string
/ nested-block-comment handling, and that a new or deleted `.rs` would surface) — it is sound, and it
is applied symmetrically to both sides.

Each diff verified by hand:

1. **`crates/lb-observability/src/xdp_metrics.rs`** — removes `_gauge_vec_anchor`:
   `#[doc(hidden)] #[allow(dead_code)] fn _gauge_vec_anchor() {}`, a private no-op function whose only
   purpose was to be a grep target for a comment. Nothing calls it; it is `#[doc(hidden)]` so not
   public API. **Behaviour-neutral.** (This also accounts for the 210→209 allow-attribute delta.)
2. **`crates/lb-quic/src/h3_bridge.rs:930`** —
   `map_err(|_| { /* comment */ RespAbort::BadHead })` → `map_err(|_| RespAbort::BadHead)`.
   The block existed only to host a comment; removing the comment left a single-expression block that
   rustfmt collapsed. **Behaviour-neutral.**
3-5. **`grpc_h3_e2e.rs`, `h3_h1_resp_stream_e2e.rs`, `h3_h3_stream_e2e.rs`** — rustfmt re-expanded
   single-line enum variants (`ServerStream { per_request: usize }` → multi-line) once the trailing
   comments that had kept them under `max_width` were removed. Verified the *only* non-blank,
   non-comment `+`/`-` lines in all three diffs are these variant reflows. **Behaviour-neutral.**

**Extending the proof to non-Rust** (the script covers `.rs` only):

- All 24 changed shell scripts: non-comment bodies **byte-identical** to `main`. (One apparent hit,
  `scripts/ci/doc-lint.sh:141`, is a `:` no-op line whose trailing comment changed — the code is `:`
  on both sides.)
- `.github/workflows/{ci,release,scheduled}.yml` and `.github/dependabot.yml`: **parsed YAML compares
  equal** for release/scheduled/dependabot. `ci.yml` differs in exactly 5 places, all of them shell
  comments *inside* `run:` blocks (cert-gen, conformance TOML, h2spec `--strict`, h3spec waiver note,
  panic-freedom grep). No step, command, `env`, `if`, matrix entry or action version changed.
- No `.md`, `.toml`, `Dockerfile`, `manifest/` or `packaging/` file changed at all.

---

## CHECK 3 — GATE-READ CONTENT

All ten strings the integration tests read out of production source, present verbatim:

| string | file:line | test that asserts it |
|---|---|---|
| `ROUND8-L7-10 — take-and-discard upstream stream pattern` | `crates/lb-l7/src/h1_proxy.rs:793` | `round8_body_overread.rs:59` |
| `set_reusable(false)` | `crates/lb-l7/src/h1_proxy.rs:802` | `round8_body_overread.rs:66` |
| `ROUND8-L7-10 — API contract for future H1 upstream reuse` | `crates/lb-io/src/pool.rs:284` | `round8_body_overread.rs:78` |
| `ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern` | `crates/lb-io/src/http2_pool.rs` | `round8_body_overread.rs:94` |
| `ROUND8-L7-05` | `crates/lb-l7/src/h1_proxy.rs` | `round8_underscore_policy.rs:83` |
| `with_header_underscore_policy` | `crates/lb-l7/src/h1_proxy.rs` | `round8_underscore_policy.rs:88` |
| `ROUND8-L7-05` | `crates/lb-l7/src/h2_proxy.rs` | `round8_underscore_policy.rs:99` |
| `with_header_underscore_policy` | `crates/lb-l7/src/h2_proxy.rs` | `round8_underscore_policy.rs:104` |
| `enable_connect_protocol()` | `crates/lb-l7/src/h2_proxy.rs:512` | `h2_connect_protocol_settings.rs:15` |
| `if self.h2_extended_connect_enabled` | `crates/lb-l7/src/h2_proxy.rs:511` | `h2_connect_protocol_settings.rs:23` |

I read the asserting tests rather than trusting the list; `round8_underscore_policy.rs` asserts on
`ROUND8-L7-05` **and** `with_header_underscore_policy` in *both* `h1_proxy.rs` (:83/:88) and
`h2_proxy.rs` (:99/:104) — four assertions, not two.

**`scripts/ci/doc-lint.sh`** — `FILES` array membership **unchanged**; only two comment lines inside
the array were removed (`# S41: …`, `# S42: …`). Script runs green: `doc-lint: OK` (tier-1 OK,
tier-2 OK, 52 Verified-Fixed claims checked).

**`scripts/ci/coverage-check.sh`** — the `REQUIRED` list is **byte-identical** to `main`, `EXEMPT` is
byte-identical, and the **entire non-comment body is byte-identical**. All three required header
passages survive:
- S44 merged-DA metric correction — lines 18-31, including the "CORRECTION, not a relaxation" argument
  and the `random.rs 86.21 → 85.71` / `conn_gate.rs 91.14 → 90.91` both-directions evidence.
- Named `loader.rs` carve-out — lines 9-13.
- The "0 > 0 is False" DA trap — lines 63-65, still directly above the `max(d.get(ln, 0), cnt)` it explains.

**`scripts/ci/h3spec-check.sh`** — non-comment body **identical**; all **12** named waivers present
(10 transport-parameter/reserved-bits at lines 38-47, 2 QPACK at 49-50), and the HONESTY CONTRACT
block (lines 13-18) survives intact.

---

## CHECK 4 — TESTS NOT SILENTLY REMOVED

Counted `#[test]` / `#[tokio::test]` attributes across every tracked `.rs` (all `crates/*/tests/`,
every `#[cfg(test)] mod tests`, every root `tests/`).

| | `main` @ ff39fa08 | `feature/de-slop-s45a` |
|---|---:|---:|
| total test attributes | **1582** | **1582** |
| files containing tests | **308** | **308** |
| per-file diff | — | **byte-identical** (only the header line differs) |

Not one test lost, in any file. This is independently corroborated by the code-identity proof: a
removed `#[test]` is a token change, and only five files have token changes — none of them a removal.

---

## CHECK 5 — SAFETY

| metric | `main` | branch | delta |
|---|---:|---:|---|
| `unsafe { … }` blocks (tracked `.rs`, excl. `audit/`) | **88** | **88** | 0 |
| per-file block counts | — | — | **identical, every file** |
| blocks with no `SAFETY` within 4 lines above | **35** | **34** | **−1 (improved)** |

The lead's 84/32↔31 differ from mine only by counting scope (mine is all tracked `.rs`); the
conclusion is the same and stronger — **per-file counts match exactly**.

The one improvement is `crates/lb-io/src/ring.rs:106`, which now has a SAFETY note within range where
`main` did not. Remaining un-noted blocks are unchanged in position and file:
`lb-l4-xdp/ebpf/src/main.rs` ×27, `lb-l4-xdp/src/netlink_xdp.rs` ×1, `lb-soak/src/gateway.rs` ×3,
`tests/h3_s3_inflight_h1_drain_proof.rs` ×1, `tests/reload_under_traffic.rs` ×2 — i.e. the sweep did
not remove a single one.

**`#[allow(...)]`:**

| metric | `main` | branch |
|---|---:|---:|
| real `#[allow]` attributes (non-comment lines) | 210 | 209 |
| allows carrying an inline `reason=` / `CLIPPY-OK` / trailing comment | **9** | **9** |

The single delta is the authorized `_gauge_vec_anchor` removal. Hard-constraint #4 holds. (The
invariant census reports −2; that regex also counts `#[allow(` appearing *inside* deleted comment
prose, which is not an attribute.)

---

## CHECK 6 — DELETED / MOVED FILES

Confirmed independently: **20** root `run-*.sh` deleted, **8** scripts moved to `scripts/archive/`
(`scripts/soak/` → 3, `scripts/perf/` → 5).

- Anchored grep for each deleted basename across **all tracked files** (workflows, scripts, docs,
  README/CONTRIBUTING, `Cargo.toml`s, `Dockerfile`, `docker/`, `manifest/`, `packaging/`, `.gitignore`),
  excluding `audit/`: **zero hits**.
- Grep for each moved script's **old path**: **zero hits**.
- Grep for each moved script's **basename**: only self-references inside its own usage string, all
  already rewritten to `scripts/archive/…`.
- **Internal cross-references inside the moved scripts:** `scripts/archive/s39-gate-feasible.sh:17`
  correctly points at `scripts/archive/s39-x3.sh` (the case the lead flagged — fixed).
  `scripts/archive/s39-burnin.sh:27` invokes `scripts/soak/run-soak.sh`, which still exists at that path.
  No moved script uses `dirname`/`BASH_SOURCE`/relative `../` — the four that `cd` do so to the absolute
  repo root, so the move cannot break them.
- Exhaustive dangling-path sweep: every path-like `*.sh` token in every tracked file resolves.
  (`docs/arch/security-and-conformance.md:93`'s `../../scripts/ci/h3spec-check.sh` resolves correctly
  from `docs/arch/`.)
- Every script the CI invokes exists: `scripts/ci/{doc-lint,coverage-check,h3spec-check,docker-smoke}.sh`,
  `scripts/release-soak.sh`, `scripts/soak/release-soak-onbox.sh`, `scripts/build-xdp.sh`,
  `scripts/never_decrypted_proof.sh`, `scripts/halting-gate.sh`.

See F-6 for the now-empty `scripts/perf/`.

---

## CHECK 1 — KNOWLEDGE-REGRESSION SAMPLE

Method: extracted the `LOAD-BEARING NOTABLES` from all five `s45a-inv-*.md`, built **146 mechanical
fact-probes** across every area (weighted to security + protocol), ran them against the *comment text*
of `main` vs the branch, then **read every flagged site by hand** plus four extra sites the mechanical
pass got wrong. 150 items adjudicated.

| verdict | count | meaning |
|---|---:|---|
| **PRESERVED** | 41 | fact present in near-original form, or verbatim (gate strings, SAFETY notes, `entry().or_insert_with()`) |
| **COMPRESSED-OK** | 105 | prose compressed hard, fact fully intact — the intended outcome |
| **LOST** | **4** | fact not recoverable from the current file |

**LOST (4):**

| # | location | fact lost | severity |
|---|---|---|---|
| 1 | `crates/lb/src/main.rs:2184` | S37-C/R6 install-signal-streams-ONCE-outside-the-loop | **should-fix** (F-1) |
| 2 | `crates/lb/src/main.rs:4716` | RFC 6455 §7.1.5 + `ws_handle_client_fin` clean-EOF-not-Reset | **should-fix** (F-2) |
| 3 | `crates/lb-l7/src/h2_proxy.rs:1109` | `// SAFETY: guarded by is_data()` | note (F-4) |
| 4 | `crates/lb-grpc/src/frame.rs:33` | `// SAFETY of .get(): len >= GRPC_HEADER_SIZE checked above` | note (F-4) |

**The five canaries the lead named — all confirmed in substance:**

- **`conn_actor.rs:374-386` (F-S29-1):** literal `entry().or_insert_with()` **present verbatim** at
  :376, with the full mechanism — *"a fresh StreamTx … replays … and would discard a large response's
  still-buffered trailer+FIN (gRPC-fatal)"* — and the `get_mut` contrast intact. **PRESERVED.**
- **`lb-security/**`:** `check_te_strict` (`smuggle.rs:110`, doc at :14), TE-must-equal-`trailers`
  (`smuggle.rs:153-154`), pseudo-header leak reject (`smuggle.rs:129`), LRU-not-FIFO
  (`zero_rtt.rs:2`, verbatim *"**LRU, not FIFO** (SEC-2-05): a FIFO lets a unique-token spray push the
  in-flight replayee out"*), HMAC-not-multiply-shift (`zero_rtt.rs:4`), HMAC-32-byte SAFETY
  (`zero_rtt.rs:21`), constant-time compare (`admin_auth.rs:3/:82`; `retry.rs:166` is now *stronger*
  than main — *"`ct_eq` over `==` is the whole point; do not 'simplify' it"*), never-render-key-material
  (`ticket.rs:59`, `admin_auth.rs:91`), conn_gate AcqRel (`conn_gate.rs:3`, :132/:144/:172) and per-IP
  overflow rollback (`conn_gate.rs:121` — *"MUST roll the per-listener counter"*, with the guard tests'
  intent kept at `tests/conn_gate.rs:64` and `tests/hooks_impl.rs:112`). **ALL PRESERVED/COMPRESSED-OK.**
- **`lb-io/src/idle_send.rs`:** `biased;` contract **COMPRESSED-OK** at :61 —
  *"Load-bearing: at the same virtual instant success MUST win over a spurious timeout (arm iv)."*
  Pinning contract **COMPRESSED-OK** at :44. The S14 CFBW-RECHECK stale-`complete` re-load fix survives
  at :71-73 with the mechanism *and* the consequence (*"Phase B — so `head_timeout` — becomes
  unreachable for small bodies"*). This file is a model of the standard done right: 12 lines of prose
  replaced ~45 with zero loss.
- **`lb-io/src/http2_pool.rs:259-263` (F-MD-4 `reset_peer`):** **PRESERVED** — *"Injecting a body
  error into hyper's `SendStream` does NOT work: hyper may END_STREAM the upstream, presenting the
  truncated body as COMPLETE, before it polls the injected error."* Plus the deliberate trade-off
  sentence. `MAX_HEADER_LIST_SIZE` F-RES-2 rationale intact at :41-42 and :281.
- **`lb-l4-xdp`:** `ptr_at` `checked_add`, aya#1562 / CVE-2022-23222, verifier-baseline refresh
  obligation, the *"Verifier will not accept an unbounded loop"* extension-header cap, RST/FIN prune
  (Cilium), Katran `is_under_flood()`, `BACKENDS_V4` scope note, RFC 791/2460 fragment guards, the
  `const _: ()` anchor block (:55-59), CODE-2-07 padding, ROUND8-L4-01 sentinel, EBPF-2-04
  EOPNOTSUPP/EINVAL literals, the 10-50× SKB "loud failure" note, ROUND8-L4-12 EBUSY, F-COR-7 ena key,
  the aya `BPF_PROG_TEST_RUN` blocker, netlink `IFLA_XDP` format + the prog-id-0 misattribution note,
  `BPF_FS_MAGIC`, ENA DRV preconditions, iproute2 non-vacuity — **all present.**
  The **loader.rs coverage carve-out** is documented in `scripts/ci/coverage-check.sh:9-13` and
  `:127-132`, byte-identical to `main`. **PRESERVED.**
  The wire-stable slot contract is if anything *clearer* now — `stats_export.rs:159-166` keeps both the
  `AttachProbeFailed = 16` "NOT a kernel per-CPU slot, but holds a wire-stable position" note and the
  ROUND8-L4-05 `NUM_SLOTS == 16` bound.

**Where my mechanical pass was wrong (worth recording):** it produced **8 false LOSSES** out of 13
flags — `biased`, `set_reusable`, `max_requests_per_h3_connection`, `CANONICAL_LABELS`,
`rebuild_l7_proxies`, `reset_peer`, `MAX_HEADER_LIST_SIZE`, the ebpf anchor — every one because good
compression *stops repeating the identifier it documents*. It also produced a **false PASS** on the
SIGNAL-LOSS item (F-1), which only reading caught. Grep alone cannot verify this class of work.

---

## BONUS: what the sweep FIXED (not asked for, worth crediting)

The scanners catalogued a large body of *actively wrong* comments. The sweep did not merely preserve
them — it removed them. Independently verified gone:

- `h3_h1_resp_stream_e2e.rs` — *"Currently FAILS … Keep failing until fixed; do NOT weaken or ignore"*
  on a test whose defect **had been fixed**. The single most misleading comment in the tree.
- `conn_actor.rs` — the `Buffered` "LEGACY shape … still serves H2/H3 round-trips" block describing a
  variant deleted at S25/INC-5, self-contradicted 10 lines later.
- `h3_bridge.rs` — *"`read_h1_response` reads the whole upstream response to EOF into one `Vec`
  (FULLY BUFFERED) so a malicious upstream could OOM the proxy"*, which directly contradicted the R8
  streaming design the rest of the file documents.
- `sni_authority.rs` / `sni_authority_mismatch.rs` — *"DEFERRED to Wave-2c"* on a validator that is
  wired on three hot paths, an open invitation to a duplicate wire-up.
- 12 of 13 stale `quiche 0.28` pins on a 0.29.1 tree (see F-5 for the deliberate survivor).
- Every dangling intra-doc reference the scanners named: `request_h3_upstream` 9→0, `check_block_len`
  7→0, `encode_h3_headers_frame` 7→0, `RespEvent::Bytes` 6→0, `StreamRxBuf` 10→0, `feed_body` 6→0,
  `drain_body_stream` 2→0, `RecvState::InSkip` 2→0, `h3_response_to_h1` 3→0,
  `translate_h1_request_to_h2` 3→0, `collect_h2_*` 5→0.

---

## CHECK 7 — READABILITY JUDGEMENT

Read end to end: `lb-core/src/shutdown.rs`, `lb-io/src/idle_send.rs`, `lb-l4-xdp/src/stats_export.rs`,
`lb-soak/src/timeseries.rs`, `lb-soak/src/loadgen.rs` (all 53 surviving comments), plus the compressed
regions of `lb-l7/src/h2_proxy.rs`, `lb-quic/src/h3_bridge.rs`, `lb-io/src/pool.rs`,
`lb-config/src/lib.rs`, `lb-observability/src/label_budget.rs`, `lb/src/main.rs`.

**Verdict: the prose reads as deliberately human-authored and high-signal. The line wrapping does not.**

### It reads as human-authored — evidence

`crates/lb-core/src/shutdown.rs:1-4` and `:54-55`:

> `//! Process-wide graceful drain … \`TaskTracker\` NOT \`JoinSet\`: per-connection handlers spawn
> their own helper futures that must be tracked alongside the parent, with no accept loop to hold the
> handles.`
>
> `/// Listener-cancel token. Accept loops MUST select on this, not [\`Self::token\`], or stopping
> accepts also cancels in-flight connections.`

Both give the *consequence of getting it wrong*, in one line. That is the shape a good engineer writes.

`crates/lb-l7/src/h2_proxy.rs:115-126` — 70 lines of F-SEC-1 mechanism compressed to 12 with nothing lost:

> `/// THE CATCH: h2 drops this io a microsecond after \`poll_shutdown\` returns \`Ready\`. Dropping a
> socket with unread inbound makes Linux emit an **RST** (RFC 1122 §4.2.2.13 / \`tcp_close\`), and the
> peer then discards its ENTIRE receive buffer — including the GOAWAY that already arrived.`
> `/// Fix: FIN FIRST (a FIN never causes an RST), THEN drain inbound until the peer closes its write
> half … Hard-bounded by BOTH [\`CleanCloseIo::DRAIN_CAP\`] and [\`CleanCloseIo::LINGER_DEADLINE\`] so a
> flooding client cannot pin a worker.`

`crates/lb-soak/src/loadgen.rs` went 426 → 53 comment lines (87.6%), and I read all 53: **every one is
a real catch.** e.g. `:1294` — *"``loop { break id }`` rather than ``Option`` + ``.expect()``:
infallible-by-construction still trips the panic-freedom deny lint (S34)"*; `:1667` — *"BINARY (0x2),
not Text: a WS Text frame MUST be valid UTF-8 (RFC 6455 §5.6) and tungstenite correctly tears the
tunnel down on non-UTF-8 Text"*; `:292` — *"CRITICAL (F-S27-2): this client READS NORMALLY … A
NON-reading client would exercise the gated H2 unbounded-buffer DoS instead, which is NOT what this
soak proves."* Zero filler survived. This is the best file in the sweep.

`crates/lb-config/src/lib.rs:143-146` compressed a 16-line block without dropping the danger:

> `/// Bounds quiche's insert-only \`StreamMap::collected\` — \`0\` RE-OPENS both the RSS-staircase leak
> and a single-connection DoS vector.`

### Where it reads as machine output — evidence

Not hollowness; **shape**. `crates/lb-l4-xdp/src/stats_export.rs:85` is one 245-column line:

> `/// Snapshot of which pinned maps were reused vs. freshly created at startup. Bit \`i\` is \`1\` if the \`i\`-th pin in [\`pin_names()\`] was reused; the packing keeps the Prom scrape a single atomic load projected to per-name gauges, no Mutex.`

Good content, but no human types a 245-column line into a file whose code is wrapped at 100. Same at
`crates/lb-soak/src/timeseries.rs:307` (200 cols), `crates/lb-l4-xdp/ebpf/src/main.rs:281` (168 cols),
`crates/lb/src/main.rs:4716` (172 cols) and 612 others. See **F-3**.

### Is any surviving one-liner now actively MISLEADING?

I looked specifically for summaries that over-claim because the qualifier was cut. **I found one:**

- `crates/lb/src/main.rs:4716` (**F-2**) — the doc now describes only what the *client* does and omits
  what the test *proves*. It is hollow rather than false (the title still says "→ abnormal close", and
  the assert string at :4745 carries the RFC), but it no longer points at `ws_handle_client_fin`, which
  is the code a future editor would break.

I checked the other high-risk collapses and found **no** over-claim: `xdp_metrics.rs:12`
correctly keeps the *"registered but NOT yet fed by an eBPF slot"* caveat on `conntrack_full_total`
(main buried it in prose); `xdp_metrics.rs:1-4` keeps the counter-decrease re-baseline rule and the
non-Linux stub rationale; `timeseries.rs:307` keeps the full 1.8×-separates-linear-from-quadratic
derivation; `pool.rs:284-286` keeps the anti-deletion instruction **and** the Pingora 0.6.0/0.8.0
evidence; `label_budget.rs:151-153` keeps *"not folded into an `"other"` bucket, because a placeholder
masks the bug class"*.

**Overall:** 73.5% reduction (28,435 → 7,537 comment lines in tracked `.rs`) with 4 lost facts out of
150 sampled, plus a meaningful correction of pre-existing misinformation. That is a good result. Fix
F-1 and F-2, re-wrap the three crates in F-3, and this is materially better than `main` on every axis
the standard names.

---

## APPENDIX — gate transcript

```
$ cargo fmt --all -- --check
FMT_EXIT=0

$ CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.68s
EXIT=0

$ bash scripts/ci/doc-lint.sh
doc-lint tier-1: OK
doc-lint tier-2 (audit-of-audit): OK (52 Verified-Fixed claims checked)
doc-lint: OK
DOCLINT_EXIT=0
```

**Clippy non-vacuity:** the result is fully cached, so I verified the cache post-dates the tree.
Newest tracked `.rs` mtime is `crates/lb-l7/src/h2_proxy.rs` @ 11:59:47; `lb-l7` clippy fingerprints
run to 12:00:45 and the newest workspace fingerprint is 12:01:19. The only commit after that
(`90a0dc97`, 12:08:33) touches `audit/` only. The green covers the current source.

**Invariant census** (`audit/craft/s45a-invariant-census.sh`) — reported for completeness. Per the
standard it is *"a review aid, not a gate"*; its keyword counts fall by construction as prose is
compressed. **Both named canaries pass.** I independently re-derived every count it flags and none is
a genuine regression except the four in the LOST table:

```
  ok    unsafe blocks   : 95 (baseline 95)
  FAIL  SAFETY comments : 56 < 61   -> 3 COMPRESSED-OK + 2 defensible (F-4). Real unsafe-block
                                       SAFETY coverage IMPROVED (35 -> 34 un-noted).
  FAIL  allow() attrs   : 184 < 186 -> counts `#[allow(` inside deleted comment prose. Real
                                       attributes 210 -> 209 = the authorized _gauge_vec_anchor.
  FAIL  RFC citations   : 317 < 547 -> de-duplication of repeated citations + removal of the
                                       stale quiche-0.28 pins. Every RFC cite in the KEEP list
                                       verified present except F-2.
  FAIL  CF- / F-S refs  : 68 < 135, 58 < 132 -> tag repetition across a block collapsed to one
                                       mention. All named CF-/F-S facts verified present.
  FAIL  h3spec refs     : 11 < 22   -> the 12 authoritative waivers live in h3spec-check.sh and
                                       are byte-identical to main.
  ok    CANARY  F-S29-1 (get_mut, not or_insert_with)
  ok    CANARY  conn_gate re-insert-on-next-admit
```
