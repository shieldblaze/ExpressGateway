# sweeper-support — S45A sweep result

Area: `crates/lb-{security,io,config,observability,core,balancer,controlplane,health,cp-client}/**`

## Headline

| metric | value |
|---|---|
| comment lines before | **5,927** |
| comment lines after | **2,167** |
| reduction | **63.4%** (3,760 lines removed) |
| files touched | 72 of 80 |
| behaviour changes | 1 authorised dead-fn removal; nothing else |

Measured with the brief's command:
`grep -rhE '^\s*(//|/\*|\*)' crates/lb-security crates/lb-io crates/lb-config crates/lb-observability crates/lb-core crates/lb-balancer crates/lb-controlplane crates/lb-health crates/lb-cp-client --include='*.rs' | wc -l`

### Why 63% and not 86%

The standard's 86.9% ceiling assumes "compress every doc block to 1 line + delete EVERY plain
comment". This area cannot reach that, for a structural reason: it is almost entirely **public API
under `#![deny(missing_docs)]`**. After the sweep:

- **1,097 lines are single-line comments** — overwhelmingly the one-doc-line-per-`pub`-item floor.
  `lb-config/src/lib.rs` alone declares ~200 public config fields, each of which must keep a line.
  This is irreducible without dropping the lint.
- **~1,070 lines sit in multi-line blocks**, now averaging 3.7 lines — mostly genuine catches
  (attack explanations, ordering constraints, known gaps) that clause 2 protects.

63.4% is what the rule allows here. I did not delete catches to close the gap.

## Per-file table

Only files whose count changed are listed, ordered by lines removed.

| before | after | cut | file |
|---:|---:|---:|---|
| 964 | 334 | 65% | lb-config/src/lib.rs |
| 273 | 92 | 66% | lb-io/src/quic_pool.rs |
| 270 | 83 | 69% | lb-security/src/ticket.rs |
| 254 | 99 | 61% | lb-core/src/shutdown.rs |
| 241 | 73 | 69% | lb-io/src/http2_pool.rs |
| 211 | 73 | 65% | lb-io/src/pool.rs |
| 169 | 52 | 69% | lb-io/src/idle_send.rs |
| 161 | 76 | 52% | lb-config/src/reload.rs |
| 144 | 39 | 72% | lb-security/src/watchdog.rs |
| 142 | 45 | 68% | lb-security/src/zero_rtt.rs |
| 138 | 39 | 71% | lb-observability/src/lib.rs |
| 136 | 58 | 57% | lb-observability/src/label_budget.rs |
| 117 | 40 | 65% | lb-security/src/retry.rs |
| 115 | 41 | 64% | lb-security/src/admin_auth.rs |
| 114 | 34 | 70% | lb-security/src/smuggle.rs |
| 114 | 70 | 38% | lb-io/src/ring.rs |
| 103 | 42 | 59% | lb-io/src/dns.rs |
| 96 | 32 | 66% | lb-observability/src/admin_http.rs |
| 95 | 50 | 47% | lb-observability/src/tracing_propagation.rs |
| 94 | 35 | 62% | lb-controlplane/src/lib.rs |
| 90 | 40 | 55% | lb-observability/src/xdp_metrics.rs |
| 85 | 16 | 81% | lb-security/src/hooks.rs |
| 84 | 34 | 59% | lb-security/src/conn_gate.rs |
| 78 | 40 | 48% | lb-io/src/sockopts.rs |
| 78 | 38 | 51% | lb-balancer/src/lib.rs |
| 75 | 30 | 60% | lb-io/src/lib.rs |
| 74 | 33 | 55% | lb-security/src/glitches.rs |
| 69 | 18 | 73% | lb-security/src/key.rs |
| 63 | 27 | 57% | lb-core/src/authority.rs |
| 62 | 22 | 64% | lb-balancer/tests/balancer_counter_sync.rs |
| 61 | 19 | 68% | lb-observability/src/log.rs |
| 60 | 18 | 70% | lb-core/tests/per_connection_drain.rs |
| 54 | 21 | 61% | lb-observability/src/probes.rs |
| 53 | 19 | 64% | lb-security/src/handshake.rs |
| 48 | 13 | 72% | lb-observability/src/quic_modeb_metrics.rs |
| 48 | 15 | 68% | lb-io/tests/miri_ring.rs |
| 48 | 8 | 83% | lb-security/src/slowloris.rs |
| 47 | 9 | 80% | lb-observability/src/quic_h3_recycle_metrics.rs |
| 43 | 14 | 67% | lb-security/tests/zero_rtt_replay_window.rs |
| 42 | 26 | 38% | lb-core/src/backend.rs |
| 40 | 12 | 70% | lb-observability/src/passthrough_metrics.rs |
| 37 | 14 | 62% | lb-balancer/src/maglev.rs |
| 37 | 10 | 72% | lb-balancer/tests/loom_atomic_counter.rs |
| 37 | 7 | 81% | lb-security/src/slow_post.rs |
| 35 | 8 | 77% | lb-security/tests/conn_gate.rs |
| 30 | 12 | 60% | lb-security/tests/tls_versions.rs |
| 29 | 12 | 58% | lb-core/tests/round8_drain_coordinator.rs |
| 29 | 10 | 65% | lb-security/tests/slowloris_watchdog.rs |
| 28 | 11 | 60% | lb-observability/tests/log_format.rs |
| 26 | 6 | 76% | lb-security/tests/smuggle_strict_te.rs |
| 25 | 20 | 20% | lb-health/src/lib.rs |
| 25 | 7 | 72% | lb-security/src/lib.rs |
| 24 | 11 | 54% | lb-balancer/src/ewma.rs |
| 24 | 9 | 62% | lb-core/tests/shutdown.rs |
| 23 | 10 | 56% | lb-balancer/src/weighted_round_robin.rs |
| 21 | 5 | 76% | lb-observability/tests/tracing_traceparent.rs |
| 20 | 14 | 30% | lb-cp-client/src/lib.rs |
| 19 | 9 | 52% | lb-balancer/src/ring_hash.rs |
| 17 | 5 | 70% | lb-observability/src/prometheus_exposition.rs |
| 16 | 4 | 75% | lb-observability/tests/metrics_xdp_slots.rs |
| 13 | 6 | 53% | lb-balancer/src/session_affinity.rs |
| 13 | 1 | 92% | lb-observability/tests/health_endpoints.rs |
| 12 | 5 | 58% | lb-observability/tests/red_label_budget.rs |
| 12 | 2 | 83% | lb-observability/tests/panic_total.rs |
| 11 | 7 | 36% | lb-balancer/src/p2c.rs |
| 11 | 4 | 63% | lb-security/tests/timeout_accept.rs |
| 11 | 3 | 72% | lb-security/tests/hooks_impl.rs |
| 9 | 6 | 33% | lb-balancer/src/weighted_random.rs |
| 9 | 3 | 66% | lb-observability/tests/metrics_xdp_conntrack.rs |
| 7 | 6 | 14% | lb-balancer/src/error.rs |
| 5 | 3 | 40% | lb-balancer/src/least_connections.rs |
| 4 | 3 | 25% | lb-balancer/src/least_request.rs |

## The corrected claims

### 1. `lb-security/src/retry.rs` — `mint()` was self-contradicting on a security path

The prose said `mint` panics via `assert!` on an over-length `odcid`; the `# Panics` section six
lines below said it never panics; the code (`mint_at`, via
`odcid.get(..odcid.len().min(RETRY_MAX_ODCID))`) silently truncates. **The code is the truth: no
`assert!` exists anywhere in the file.** Now:

> Mint a retry token binding `peer` and `odcid`. Never panics: an `odcid` longer than
> [`RETRY_MAX_ODCID`] is SILENTLY TRUNCATED (the wire length field is a `u8`), so a caller holding
> untrusted ODCID bytes must reject over-length input itself rather than rely on `verify`
> round-tripping what it passed in.

### 2. `lb-observability/src/admin_http.rs` — "No TLS, no auth" was stale

`serve_with_auth` / `AdminAuthGate` landed in that same file. Now:

> Admin HTTP listener: `GET` on `/metrics`, `/healthz`, `/livez`, `/readyz`, `/startupz`.
>
> NO TLS and NO mTLS. Bearer-token auth is OPTIONAL — [`serve_with_auth`] enforces it on
> information-bearing endpoints, while [`serve_with_probes`] serves everything anonymously. Even
> with a token the transport is plaintext, so the expected posture is a loopback bind behind a
> reverse proxy or a management VPN.

### 3. `lb-balancer/src/lib.rs` — EWMA "is updated in lb-l7" was false

Grep-verified: `set_latency_ns` and `latency_ewma_ns =` have **zero writers** outside lb-balancer
and lb-core (the two `lb-quic/src/passthrough.rs` hits are struct-literal `: 0` initialisers).

I checked `Ewma::pick` and the real consequence is **sharper than the inventory reported**. With
every backend at 0, `max_observed_latency` is 0, so `cold_start_latency` falls to `1` and every
backend takes the cold-start branch — the score becomes `1 * (active_connections + 1)`. It does not
"score equally"; it silently becomes **least-connections**. Documented on the field:

> EWMA latency in nanoseconds.
>
> NEVER WRITTEN IN PRODUCTION: nothing outside this crate and `lb-core` assigns to it or calls
> `set_latency_ns`, so it is 0 for every backend at runtime. [`ewma::Ewma::pick`] then takes its
> cold-start branch for all of them and the score collapses to `active_connections + 1` — i.e.
> selecting `LbPolicy::Ewma` silently gives you least-connections. Feeding this from the
> response-completion path is unimplemented.

…and repeated in the `ewma.rs` module header so a reader of the algorithm sees it too.

## Additional corrections I made (not on the approved list)

Three more docs asserted behaviour the code does not have. Compressing them without fixing them
would have re-blessed a false statement, so I corrected them; all are grep-verified and
comment-only. **Flagging for lead review since they were not pre-approved:**

1. **`lb-balancer/src/lib.rs` — `sync_from_state` has no production caller.** The old doc said
   "production call-sites call it before each pick". Only `tests/balancer_counter_sync.rs` calls
   it. Now documented as a KNOWN GAP on `Backend`.
2. **`lb-health/src/lib.rs` — the checker is never driven.** `record_success` / `record_failure`
   have no callers outside the crate's own unit tests, so every checker the binary seeds stays
   `Unknown` forever. Added to the module header.
3. **`lb-cp-client/src/lib.rs` — the module doc claimed it connects and exchanges config.**
   `connect()` performs no I/O; it checks `endpoint.is_some()` and sets a bool. Re-headed as a
   "client SHELL" with the no-transport fact stated. (The crate is also unreachable from the
   binary — scanner-support's STUB-DEAD finding. I documented the substance, not the dead-crate
   verdict, which is an owner decision.)

## Approved edits — status

- **DELETED** `lb-observability/src/xdp_metrics.rs` `_gauge_vec_anchor()` + its doc together.
  Zero callers; private, so `missing_docs` never applied. `cargo check -p lb-observability`
  passes (the one build I ran, per the exception in the brief).
- **DONE, clause-scoped** `lb-security/tests/tls_versions.rs`: dropped
  "Sec was OK with the single-line touch in `ticket.rs`;" and kept the
  `build_server_config_with_policy` shadowing rationale intact.
- All three stale claims corrected as above.

## Security catches preserved (compressed, never dropped)

Every attack-explaining fact in lb-security survives. Named examples:

- **zero_rtt.rs** — LRU-not-FIFO (a FIFO lets a unique-token spray push the in-flight replayee out
  of the window) and HMAC-not-multiply-shift (source-visible seeds allowed precompute collisions).
  Both promoted into the module header where they are unmissable. `// SAFETY:` on the HMAC 32-byte
  invariant untouched.
- **smuggle.rs** — TE-must-equal-`trailers` under RFC 9113 §8.2.2, the pseudo-header leak reject,
  the strict-TE codec-chain rationale (upstreams mis-decode → body-length mismatch across the
  gateway), and the "SmuggleTECL is reused deliberately; changing it is an API break" note.
- **conn_gate.rs** — the per-IP-overflow listener-counter ROLLBACK ("without it a sustained
  over-cap stream silently erodes the listener cap") and the SEC-2-16 AcqRel-because-it-gates-a-
  security-decision reason. RST-without-response = no amplification lever, kept in three places.
- **retry.rs** — constant-time MAC compare with an explicit "do not simplify it".
- **ticket.rs** — forward-secrecy collapse argument, "Never render key material", and the REL-2-03
  reload contract (failed reload keeps the old bundle; `not_after` is WARN-ONLY because refusing
  near-expiry certs is exactly wrong during an emergency rotation).
- **admin_auth.rs** — SHA-256-at-rest, constant-time compare, and "never render the digest —
  routine logging invites grep-then-reuse".
- **watchdog.rs** — the F-RES-5 SCOPE block (DETECTS, does not ENFORCE; `SlowRate` dormant by
  design; the real bounds live in the timeout stack), 63 lines → 6.
- **http2_pool.rs** — F-MD-4 `reset_peer` (why dropping the PeerEntry beats injecting a body
  error: hyper may END_STREAM the truncated body as complete first) and the ROUND8-L7-10
  broad-eviction policy with the Pingora 0.6.0/0.8.0 precedent.
- **pool.rs** — the "do not delete `set_reusable` without wiring a caller" anti-deletion
  instruction, verbatim pinned string intact.
- **lb-config** — `0` re-opens the H3 `StreamMap::collected` leak / single-connection DoS vector;
  the CVE-2022-30592 min-DCID floor; the CVE-2023-44487 / CVE-2024-27316 glitch weights; the
  underscore auth-bypass primitive; the "absent CA bundle does NOT disable verification" note.

Also preserved per the brief: every `// SAFETY:` in `lb-io/src/ring.rs` (11) and `sockopts.rs` (2),
all 21 `#[allow(...)]` justifications, and the `biased;` + pinning contract in `idle_send.rs`.

## Things I refused to cut

1. **`lb-io/src/quic_pool.rs` `per_peer_max_enforced` narrative (~40 lines).** It honestly says the
   test does not test what its name claims — synthetic conns are never `is_established`, so
   `PooledQuic::drop` never runs its bound check. Deleting it would leave a test whose name asserts
   a bound it never exercises. Compressed to 5 lines under a "HONEST LIMITATION" heading; the same
   applies to `total_max_enforced`. **The real fix is the test, not the comment** — still a finding
   for the lead.
2. **`lb-io/src/ring.rs` (38%, the weakest cut in the area).** 11 of its comment blocks are
   `// SAFETY:` justifications for `unsafe` io_uring pushes. Mandatory, untouched.
3. **`lb-core/src/backend.rs` (38%).** The AcqRel-vs-Relaxed asymmetry looks like an inconsistency
   a future editor would "fix" in either direction; the reason it is deliberate has to stay.
4. **`lb-config` per-knob validation ranges.** Compressed hard but the operator-facing WHY behind
   each bound survives (why the floor, why the ceiling) — those are what stop a future editor
   "simplifying" a range into a foot-gun.
5. **`lb-security/tests/tls_versions.rs:115`** `AsyncWriteExt::shutdown(&mut stdout())` with
   "keep tokio happy on CI". Kept: removing it orphans an otherwise inexplicable line. The LINE is
   the problem, not the comment — flagged again for a later reviewer.

## Mechanical pass worth noting

74 `# Errors` sections whose body only restated the return type (`See [`FooError`].`,
`As [`serve_with_probes`].`) were deleted outright — 4 lines each. Fact-carrying bodies were folded
into the summary line first so nothing was lost. **Lint-safety verified before doing this:**
`clippy::missing_errors_doc` is pedantic, all nine crates carry
`#![allow(clippy::pedantic, clippy::nursery)]`, and nothing in `crates/`, `Cargo.toml`, `.cargo/`,
`scripts/` or `.github/` re-enables it. Five substantive `# Errors` sections remain.

## Verification done

- `cargo fmt` across all nine crates — clean.
- Both test-pinned ROUND8-L7-10 strings confirmed present after every edit.
- Scripted check that no `pub` item lost its last doc line (the 30 hits are all `pub mod`
  declarations documented by their own `//!` inner docs — pre-existing and correct).
- `cargo check -p lb-observability` for the single code deletion.
- **NOT run** (per the brief, the lead gates centrally): build, clippy, test.

---

## Mandatory self-check proof (lead-required)

### 1. Attribute audit — no code attribute lost

```
$ git diff main -- crates/lb-security crates/lb-io crates/lb-config crates/lb-observability \
    crates/lb-core crates/lb-balancer crates/lb-controlplane crates/lb-health crates/lb-cp-client \
  | grep -E '^-\s*#\['
      1 -#[doc(hidden)]
      1 -#[allow(dead_code)]
```

Exactly two removed attribute lines in the entire area, both belonging to the authorised
`_gauge_vec_anchor` deletion and both in that single hunk. `xdp_metrics.rs` is the only file with
any removed attribute. No `#[inline]`, `#[cfg]`, `#[derive]`, `#[must_use]` or `#[test]` was lost.
`#[allow(clippy::too_many_arguments)]` in `http2_pool.rs` survived the doc-block collapse above it.

### 2. Code-identity proof

```
$ python3 audit/craft/s45a-code-identity.py main
S45A code-identity proof — 231 .rs files changed vs main
  2 file(s) with real code changes — each needs justification:
    CODE DIFFERS   crates/lb-observability/src/xdp_metrics.rs
    CODE DIFFERS   crates/lb-quic/src/h3_bridge.rs
```

- **`xdp_metrics.rs` — mine, INTENDED.** Comment-stripped diff is exactly the authorised deletion:
  ```
  @@ -126,7 +126,4 @@
  -#[doc(hidden)]
  -#[allow(dead_code)]
  -fn _gauge_vec_anchor() {}
  ```
- **`h3_bridge.rs` — NOT in my area.** `crates/lb-quic` belongs to the lb-quic sweeper; that is
  commit `1f638a2f` (author `proto`). Verified none of my commits touch lb-quic.

### 3. Test-pinned string intact

```
$ grep -n "ROUND8-L7-10 — API contract for future H1 upstream reuse" crates/lb-io/src/pool.rs
301:    /// **ROUND8-L7-10 — API contract for future H1 upstream reuse.** No production caller today

$ grep -n 'ROUND8-L7-10 — API contract' crates/lb-l7/tests/round8_body_overread.rs
88:        src.contains("ROUND8-L7-10 — API contract for future H1 upstream reuse"),
```

The h2 pin `ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern` is likewise present in
`http2_pool.rs`.

### 4. SAFETY / unsafe in lb-io

| file | SAFETY main→now | code `unsafe` sites main→now |
|---|---|---|
| ring.rs | 11 → 11 | 13 → 13 |
| sockopts.rs | 2 → 2 | 2 → 2 |
| lib.rs | 1 → 1 | 1 → 1 |
| tests/miri_ring.rs | 1 → 1 | 1 → 1 |

A raw `grep -c unsafe` on `ring.rs` reads 13→14, but the extra hit is **prose in the new module
doc** ("that synchronicity is what makes the `unsafe` pushes below sound"), not a code site.

Coverage checked directly rather than by comparing bare counts:

```
TOTAL code-level unsafe sites: 17, covered by a SAFETY/# Safety within 6 lines: 17
Uncovered sites (identical set on main — pre-existing):   <none>
```

The apparent 11-SAFETY-vs-13-unsafe gap in `ring.rs` is not a gap: the two `unsafe fn`
declarations (`push_sqe`, `sockaddr_storage_to_socketaddr`) carry `# Safety` doc sections rather
than inline `// SAFETY:` comments.

The only `SAFETY`-bearing line removed anywhere in lb-io:

```
$ git diff main -- crates/lb-io | grep -E '^-.*SAFETY'
-    // SAFETY rationale: we own `send_fut` by value, then immediately pin
```

That is `idle_send.rs`, a file with **zero** `unsafe`, where the prefix was decorative; the actual
pinning-contract fact was preserved. Lead-ruled correct.
