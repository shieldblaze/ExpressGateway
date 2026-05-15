# Round-8 fix-back re-verification (verify, task#74, 2026-05-15)

Re-check of the 6 items pushed back in the first verification pass as
hollow (zero callsites / fake tripwire / stub). Author = div-l7 /
div-l4; verifier = verify (author != verifier). Verification only —
no fixes applied.

Finding format: SHA / proof-test result / adversarial probe + outcome
/ verdict / blocking-for-prod flag.

---

## ROUND8-L7-09 — authority::validate wiring

- **SHA**: bf22f01a
- **Proof test**: `cargo test -p lb-l7 --test round8_authority_enforced`
  -> 4/4 PASS (h1/h2 comma-rejected-before-upstream, h1/h2 valid-passes).
- **Call-site read**: H1 `h1_proxy.rs:880` and H2 `h2_proxy.rs:764`
  DO call `crate::authority::validate` on Host / :authority BEFORE
  `pick_info()` and reject -> 400. Confirmed on the *normal* request
  path.
- **Adversarial probe** (can a comma in :authority reach upstream via
  a path that skips the call site?): **YES — real bypass found.**
  1. H1 WebSocket upgrade: `h1_proxy.rs:803-809` returns
     `self.handle_ws_upgrade(req)` BEFORE the L7-09 validator at line
     880. `handle_ws_upgrade` (line 1235) calls
     `self.picker.pick_info()` at 1246 and dials upstream at 1276
     with NO `authority::validate`.
  2. H2 extended-CONNECT WS: `h2_proxy.rs:645-651` returns
     `handle_ws_extended_connect(req)` before the validator at 764;
     that handler picks upstream at line 954 unvalidated.
  3. H2 gRPC: `h2_proxy.rs:652-672` reaches
     `Arc::clone(gp).handle(req, backend.addr)` (upstream) at line
     671 before the validator at 764.
  The proof test exercises ONLY plain GET requests — it does not
  cover any upgrade / gRPC path, so 4/4 PASS does not prove the
  bypass closed.
- **Verdict**: **PARTIAL — Verified-Fixed on the normal request
  path, still-Push-back on the WS-upgrade / extended-CONNECT / gRPC
  paths.** The "every parser path" claim does not hold. Validator
  must be hoisted above the upgrade/gRPC intercepts (or duplicated
  into the three handlers).
- **Blocking for prod**: NON-BLOCKING. Finding is medium and
  explicitly notes no host-based routing exists today (comma is a
  future-routing primitive, not directly exploitable now). Push-back
  stands for correctness/completeness, not as a prod blocker.

---

## ROUND8-L7-07 — GlitchesCounter per H2 connection

- **SHA**: b4a1a971
- **Proof test**: `cargo test -p lb-l7 --test round8_glitches_enforced`
  -> 1/1 PASS.
- **Adversarial probe** (does the counter actually trip the
  connection or just increment?): **It genuinely trips.**
  `GlitchConnState::record()` on `GlitchOutcome::Drain` calls
  `self.drain.cancel()` (the `conn_cancel` child token); the biased
  `cancel_fut` select arm in `serve_connection_with_cancel_sni`
  resolves and calls `conn.graceful_shutdown()` (two-step GOAWAY).
  Not just an increment.
- **Metric registration grep**: `h2_glitches_total` is registered
  via `lb-observability` `counter()` -> a real `prometheus::IntCounter`
  in a `Registry` (not a no-op). 5 real abuse callsites confirmed in
  `h2_proxy.rs` lines 692/759/794/811/851 (underscore-reject,
  HPACK-ratio, 3x rapid-reset).
- **Deferral honesty** (does hyper 1.9 really have no per-frame read
  hook?): **Honest.** `Cargo.lock` confirms hyper pinned at 1.9.0;
  `http2::Builder::serve_connection` exposes no per-frame inbound
  read context. `audit/deferred.md` defers ONLY the
  `FrameRecvTimeout` `tokio::Interval` sub-part and documents
  keep-alive PING as partial coverage. Accurate, not theater.
- **Verdict**: **Verified-Fixed.**
- **Blocking for prod**: NON-BLOCKING (deferred sub-part documented;
  counter half is live).

---

## ROUND8-L4-02 + ROUND8-L4-08 — NUM_SLOTS sibling asserts

- **SHA**: d4d81e40
- **Proof tests**:
  `cargo test -p lb-l4-xdp --test round8_conntrack_state` -> 7/7 PASS;
  `--test round8_fragments` -> 6/6 PASS.
- **Adversarial probe** (are asserts enum-derived, or just bumped
  15->16?): **Enum-derived, not bumped.** Both files now assert
  `(StatSlot::<prune/frag> as usize) < NUM_SLOTS` AND
  `StatSlot::NewFlowRateCap as usize + 1 == NUM_SLOTS`. Appending a
  new `StatSlot` after `NewFlowRateCap` and bumping `NUM_SLOTS` makes
  `NewFlowRateCap+1 == NUM_SLOTS` FAIL (NewFlowRateCap is no longer
  the last variant) — the exact anti-rot tripwire the bare literal
  lacked. Won't silently rot on the next slot add.
- **Verdict**: **Verified-Fixed (both L4-02 and L4-08).**
- **Blocking for prod**: NON-BLOCKING (test-only repair; eBPF prune /
  fragment detection logic was already correct).

---

## ROUND8-L4-05 — aya-version Cargo.lock tripwire

- **SHA**: 5600ee95
- **Proof test**: `cargo test -p lb-l4-xdp --test round8_attach_probe`
  -> 10 PASS + 1 ignored (kernel scaffold).
- **Independent /tmp simulation** (required — not trusting the
  in-test catch_unwind):
  - Copied the real workspace `Cargo.lock` to `/tmp/l405sim/`.
  - Created a bumped copy changing ONLY the `[[package]] name =
    "aya"` block from `version = "0.13.1"` to `"0.14.0"` (verified
    sibling `aya-obj` left at `0.2.1`).
  - Reproduced the tripwire's exact `aya_version_from_lock` +
    `assert_eq!` + `semver_tuple` logic in a standalone `rustc`
    binary and ran it against both lockfiles.
  - **RESULT — real Cargo.lock**: parser -> `"0.13.1"`,
    `eq_assert_passes=true`, `semver_guard_passes=true` => tripwire
    **PASSES** (no upgrade).
  - **RESULT — bumped Cargo.lock**: parser correctly extracts
    `"0.14.0"` (NOT the sibling `aya-obj`),
    `assertion left == right` panics, `eq_assert_passes=false`,
    `semver_guard_passes=false` => tripwire **FAILS** (upgrade
    detected).
- **Did the L4-05 tripwire actually fail under the simulated aya
  0.14 bump? YES.** It is a real string-parse + semver detector, not
  the old const-fn tautology.
- **Verdict**: **Verified-Fixed.**
- **Blocking for prod**: NON-BLOCKING (kernel BPF_PROG_TEST_RUN probe
  deferred behind documented aya API blocker; static NIC blocklist
  remains the live defence).

---

## ROUND8-L4-12 — RTM_GETLINK netlink query + detach_verifying

- **SHA**: 67024106
- **Proof test**:
  `cargo test -p lb-l4-xdp --test round8_netlink_xdp_query` -> 7 PASS
  + 1 ignored (live AF_NETLINK lane).
- **detach_verifying read** (`loader.rs:1474`): NO LONGER a
  `prog_id: None` stub. Step 1 real `query_xdp` pre-check (rejects
  `ForeignProgramAttached` / `NoProgramAttached`); Step 2 real
  `xdp.detach(link_id)` using the retained `XdpLinkId` (line 1501 —
  the empty-block gap is closed); Step 3 real post-detach
  `query_xdp` returning `DetachLeftProgramAttached` if a prog
  survives.
- **Adversarial probe** (malformed/truncated netlink response — does
  it fail safe?): **Yes, fails safe.** A truncated/garbled response
  makes `parse_getlink_response` return `Err(InvalidData)`
  (`netlink_xdp.rs:186-194`); `query_xdp` wraps it as
  `XdpLoaderError::XdpQueryFailed` and propagates via `?` through
  `detach_verifying` — the operation FAILS, it does NOT falsely
  report "detached". Confirmed by tests
  `truncated_message_is_rejected_not_panicked` (is_err) and
  `nlmsg_error_with_errno_is_surfaced`. Caveat noted: a *clean*
  NLMSG_DONE with no RTM_NEWLINK yields `XdpLinkInfo::default()`
  (prog_id None) — acceptable, that is a non-malformed empty kernel
  reply, distinct from the truncation case.
- **Verdict**: **Verified-Fixed.**
- **Blocking for prod**: NON-BLOCKING (single-syscall BPF_F_REPLACE
  genuinely unavailable in pinned aya 0.13.1; detach-then-attach
  under the now-real pre-check is the correct dependency-floor
  mitigation).

---

## Summary table

| Item | SHA | Proof | Verdict | Prod |
|---|---|---|---|---|
| L7-09 | bf22f01a | 4/4 | **PARTIAL — still Push-back (WS/gRPC bypass)** | non-blocking |
| L7-07 | b4a1a971 | 1/1 | Verified-Fixed | non-blocking |
| L4-02 | d4d81e40 | 7/7 | Verified-Fixed | non-blocking |
| L4-08 | d4d81e40 | 6/6 | Verified-Fixed | non-blocking |
| L4-05 | 5600ee95 | 10+1 | Verified-Fixed (tripwire FAILS under simulated aya 0.14) | non-blocking |
| L4-12 | 67024106 | 7+1 | Verified-Fixed | non-blocking |

5 of 6 genuinely closed (not push-back-theater). L7-09 closed the
normal request path but the adversarial probe found a real residual
bypass on the WebSocket-upgrade / H2 extended-CONNECT / gRPC paths
that skip the validator before upstream selection — push-back stands
(non-blocking, medium, future-routing primitive).

---

## ROUND8-L7-09 re-re-verify (verify, task#76, 2026-05-15, sha 1a89a4e4) — VERIFIED-FIXED (H1/H2 scope) + NEW finding ROUND8-L7-16 (H3 gap)

Re-checked div-l7's `1a89a4e4` (I did not author it).

### 1. Choke-point placement — CONFIRMED
- `crate::authority::validate_request<B>(&Request<B>)` (authority.rs:78-128)
  validates BOTH `req.uri().authority()` (H2 `:authority` / H1 absolute-form)
  AND the `Host` header; non-empty present values only; reject -> `(String, AuthorityError)`.
- H1: it is the **FIRST statement** of `H1Proxy::handle_inner`
  (`crates/lb-l7/src/h1_proxy.rs:779`). `handle` (728) only opens the
  trace span then calls `handle_inner`. The gRPC-415 reject (802) and
  the WS-upgrade fork are strictly BELOW 779.
- H2: it is the **FIRST statement** of `H2Proxy::handle_inner`
  (`crates/lb-l7/src/h2_proxy.rs:654`), ABOVE the RFC-8441
  extended-CONNECT intercept (672) and the gRPC fork (679). `handle`
  (603) only opens the span / injects trace ctx.
- The OLD redundant in-path L7-09 blocks were REMOVED in both files
  (replaced by comments at h1_proxy.rs:902-905 / h2_proxy.rs:790-794).
  No double-validation, no dead path.

### 2. round8_authority_enforced — 7/7 PASS
The 3 new tests drive REAL request shapes down each previously-bypassing fork:
- `test_ws_upgrade_comma_authority_rejected`: structurally-valid RFC 6455
  H1 handshake (Upgrade/Connection/Sec-WebSocket-Key) + `Host: a,b` -> 400,
  asserts real accept-counting probe backend `hits == 0`.
- `test_h2_ext_connect_comma_authority_rejected`: real H2 handshake,
  `CONNECT` + `hyper::ext::Protocol("websocket")` + comma `:authority`
  -> 400, probe backend `hits == 0`.
- `test_h2_grpc_comma_authority_rejected`: real H2 handshake, POST
  `content-type: application/grpc` + `te: trailers` + comma `:authority`
  -> 400, probe backend `hits == 0`.
Each uses a real `TcpListener` probe with an `AtomicU32` SeqCst counter
and asserts ZERO upstream connections. Test shapes match their names —
not plain-GET-renamed.

### 3. Adversarial 4th-path hunt
- H2->H1 downgrade bridge (`proxy_request`, h2_proxy.rs:918): downstream
  of the line-654 choke point — COVERED.
- Non-extended CONNECT on H2: lacks the `:protocol` extension so
  `is_h2_extended_connect` is false; falls through to the normal path
  below the choke point — COVERED.
- Pipelined / kept-alive H1 second request: hyper invokes
  `handle` -> `handle_inner` PER request, so the choke point runs on
  every request — COVERED.
- **H3/QUIC: SEPARATE UNGUARDED DISPATCH — REAL GAP.**
  `lb-quic` is a distinct crate that does NOT depend on `lb-l7`.
  `crates/lb-quic/src/conn_actor.rs:361` builds
  `H3Request::from_headers(headers)` and proceeds directly to upstream
  selection (`h2_backend` 364 / `h3_backend` 374 / `select_backend` 386)
  with NO `authority::validate` anywhere. The `:authority` pseudo-header
  (`h3_bridge.rs:121-131`) is forwarded verbatim into `build_h1_request`
  (h3_bridge.rs:139-147) and the H2/H3 upstream builders. `grep -rn`
  for `authority::validate|forbid comma|BUG/MAJOR` across `lb-quic` and
  `lb-h3` returns ZERO hits. This is exactly the HAProxy `BUG/MAJOR`
  desync primitive — and L7-09's own Recommendation step 2 explicitly
  required "H3: in the QUIC conn_actor's header-callback path", which
  the fix did NOT deliver.

### 4. round8_drain_15case — 16/16 PASS (unchanged by this fix)

### VERDICT
- **ROUND8-L7-09: VERIFIED-FIXED** for its H1/H2 scope. The 3 forks
  (H1 WS-upgrade, H2 ext-CONNECT, H2 gRPC) now reject -> 400 with zero
  upstream connections; no 4th bypass on the H1/H2 path.
- **NEW finding ROUND8-L7-16 opened** for the H3/QUIC unguarded
  dispatch (`crates/lb-quic/src/conn_actor.rs:361`). Severity medium,
  NON-BLOCKING for prod (future-routing primitive, same risk class as
  L7-09's pre-fix state; no host-based routing today) but it MUST be
  closed before any vhost/host-routing pillar lands, same as L7-09.
  Reported to lead.
