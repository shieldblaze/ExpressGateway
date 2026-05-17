# builder-1 — Consolidated Fix Plan (F-SEC-1, F-COR-1, F-COR-6)

Serialized chain (shared files h2_proxy.rs / h3_bridge.rs). Implemented
strictly in order, one commit each, NOT pushed (lead pushes). Author !=
verifier — verifier (different agent) verifies separately.

NOTE ON COMMS: no SendMessage/TaskUpdate tool exists in this
environment. I am using this `plans/` file for the plan and
`messages/builder-1-to-lead.md` (append-only log) for checkpoints. If
lead expects a different channel, please reply there. I WAIT for
`messages/lead-to-builder-1.md` to contain "approved" before any source
edit.

---

## Finding 1 — F-SEC-1 (SECURITY) — Rapid-Reset GOAWAY lost under load

### Root cause (confirmed from source, matches findings-auditor-2 A2-1)
`crates/lb-l7/src/h2_proxy.rs:515` — `res = &mut conn` select arm.
Rapid-reset enforcement is delegated to hyper/h2 via
`H2SecurityThresholds::apply` (max_pending_accept_reset_streams /
max_local_error_reset_streams). On trip, hyper QUEUES a
GOAWAY(PROTOCOL_ERROR) into its write buffer and resolves the conn
future `Err`. The current arm returns immediately on that `Err` and
drops `conn` (and its TLS/TCP `io`) WITHOUT awaiting the buffered
GOAWAY flush. Under concurrent-runtime scheduler starvation TCP FIN/RST
beats the buffered GOAWAY → client sees `Io(BrokenPipe)`, never the RFC
9113 §6.8 protocol signal. The SIGTERM/glitch arm (:498-513) already
does this correctly via `graceful_shutdown()` + bounded
`timeout(total, conn)`.

### Fix
In the `res = &mut conn` arm, when the connection future resolves
`Err(e)` (hyper's internal rapid-reset / local-error-reset trip), do
NOT drop `io` immediately. Drive a BOUNDED GOAWAY flush before
teardown, mirroring the existing cancel arm's discipline:

- On `Err(e)`: call `conn.as_mut().graceful_shutdown()` then
  `tokio::time::timeout(<bounded budget>, &mut conn).await`, ignoring a
  second error/timeout (the GOAWAY is already queued; this just lets
  hyper poll the write side to flush the buffered frame to the socket
  before we drop `io`). Return the original error semantics
  (`Err(io::Error::other("h2 server: {e}"))`) so existing callers/tests
  that key on connection failure are unaffected — the ONLY behavioral
  change is that the queued GOAWAY is now actually written.
- Budget: a small fixed bound (e.g. `min(total, 5s)`) so a wedged
  client cannot delay teardown; the flush is buffered-write-only so it
  completes in microseconds when the scheduler is not starved and is
  hard-bounded when it is. Exact constant called out in the diff for
  verifier review.
- `Ok(())` path unchanged.

Rationale for `graceful_shutdown()` even though hyper already queued
its own GOAWAY: `graceful_shutdown` is idempotent w.r.t. an
already-queued GOAWAY (it sets the soft limit + drains); its real job
here is to give us a clean "poll conn until writes flush or bounded
timeout" without re-implementing hyper's write pump. If
`graceful_shutdown` after an errored conn proves to misbehave in
testing, fallback is to poll `&mut conn` (already fused to `Err`) under
the timeout purely to drive the write side — I will verify which
actually flushes the frame in the regression and document the choice
in the commit message.

### Regression test (mandatory, SECURITY tier)
New test file `tests/h2_rapid_reset_goaway_under_load.rs` (separate
binary so it can self-spawn contention without poisoning sibling
suites). It:
1. Spawns the real H2s listener (reuse the harness shape from
   `tests/h2_security_live.rs`: tightened thresholds
   max_pending_accept_reset_streams=3 / max_local_error_reset_streams=3).
2. Induces the *exact* contention mechanism auditor-2 proved is
   required (pure spin does NOT repro): N parallel child processes each
   running an independent multi-thread tokio runtime doing the
   rapid-reset flood, PLUS CPU spin workers, PLUS a disk-churn loop
   (bounded, temp-file, cleaned up). Implemented as an in-test harness:
   the test binary re-execs copies of itself (`std::env` flag) as the
   flood workers + spawns spin threads + a bounded `dd`/fsync churn
   task, so a single `cargo test` invocation reproduces the
   concurrent-multi-runtime starvation in-process-tree.
3. Each flood worker asserts the client observes a server-initiated
   GOAWAY: `err.is_go_away() || err.is_remote()` / conn future resolves
   `GoAway(_, PROTOCOL_ERROR, Remote)` — and explicitly FAILS on
   `Io(BrokenPipe)` (the defect signature).
4. Aggregates: ALL workers must see a real GOAWAY; ANY BrokenPipe =
   test failure with the captured `conn_res`.

Pre-fix demonstration: I will run this test against the UNFIXED tree
first and capture a failing run (BrokenPipe observed) into
`messages/builder-1-to-lead.md`; then apply the fix and capture
green ×3. (If a single run is flaky pre-fix due to scheduling, the
test loops the flood enough rounds that the defect is reproduced with
high probability under the induced churn — auditor-2 got 6/48; the
in-test harness raises worker count + rounds so pre-fix failure is
reliable, and I will show the captured BrokenPipe.)

### Risks
- `graceful_shutdown` on an already-errored hyper conn: mitigated by
  the poll-`&mut conn` fallback + bounded timeout; verified empirically
  in the regression (must show real GOAWAY post-fix).
- Test self-exec/contention harness must clean up children + temp
  files even on panic (RAII guard / `Drop`). Bounded disk churn (small
  file, capped iterations) — no unbounded disk use on the shared box.
- Targeted test runs only (not `--all-features` workspace) to avoid
  contending with sibling agents per the process brief.

---

## Finding 2 — F-COR-1 (CONFORMANCE) — validation-vs-forward race +
## missing no-pseudo-header-in-trailers (H2 and H3)

### Root cause (confirmed from source, matches A2-2)
Two coupled defects:

(a) **Ordering race.** For H2→H1 (`proxy_request`, h2_proxy.rs:1034-
1075) the live H2 inbound `IncomingBody` is handed straight into
`sender.send_request(req)`; hyper's H1 client streams it upstream. The
static backend replies `200 "ok"` immediately. The gateway relays that
backend body BEFORE hyper/h2 finishes protocol validation of the
malformed inbound stream (trailers / stream-state / content-length /
second-HEADERS / flow-control). h2spec sees `DATA(2)` instead of the
mandated RST/GOAWAY. Five h2spec faces, one fails per run
nondeterministically; the winner of the validation-vs-forward race.

(b) **Missing rule.** No-pseudo-header-in-trailers (RFC 9113 §8.1 /
RFC 9114 §4.3) is absent everywhere in crates/. H2 trailer-capture
sites build `trailers_vec` with NO `starts_with(':')` filter
(h2_proxy.rs:1168 in `translate_h2_request_to_h2`, :1386 in
`collect_h2_request_to_h3_fieldlist`) — contrast the regular-header
path `h2_to_h2.rs:19`. H3 trailer decode (h3_bridge.rs:382-388,
`feed_body` `InTrailers` arm) pushes `BodyItem::Trailers(trailers)`
with no pseudo check.

### Fix (a) — gate forward on inbound validation completion
In `proxy_request` (the H2→H1 path — the one h2spec exercises, static
H1 backend), fully consume + validate the inbound H2 request body
BEFORE dialing/forwarding upstream: replace the streamed-body forward
with an explicit `req.into_body().collect().await` (driving hyper/h2
protocol validation to completion and surfacing its error), then
rebuild the upstream request with the buffered body (+ captured
trailers). On `collect()` `Err` → return a synthetic error that makes
hyper reset/teardown the inbound stream WITHOUT having emitted the
backend body (return `ProxyErr::Upstream`/an error response BEFORE any
backend dial; the malformed-request rejection is then hyper/h2's
queued stream error, not a 200 DATA leak). This removes the race window
entirely: no backend dial happens until the inbound request is fully
received and validated.
  - The H2→H2 (`translate_h2_request_to_h2`) and H2→H3
    (`collect_h2_request_to_h3_fieldlist`) paths ALREADY
    `body.collect().await` before forwarding, so they are not
    race-vulnerable for body; they still need fix (b).
  - Memory: buffering the request body changes H2→H1 from streamed to
    buffered for the request direction. To avoid an unbounded-buffer
    regression I will cap the collect at the existing request-body
    ceiling used elsewhere (reuse `MAX_REQUEST_BODY_BYTES`-equivalent
    on the H2 side if present; if no H2 cap constant exists I will
    introduce a single named constant mirroring the H3
    `MAX_REQUEST_BODY_BYTES`, and 413 on exceed). I will confirm the
    exact constant/threshold in the diff for verifier. This matches
    how the H2→H2/H2→H3 paths already behave (they collect()), so it
    is consistent, not a new risk class.
  - **Open question for lead (R7-adjacent, flagging not escalating):**
    buffering the H2→H1 request body is the minimal correct fix that
    closes the race deterministically and matches the sibling H2→H2/
    H2→H3 paths' existing behavior. A fully-streaming alternative
    (forward incrementally but withhold the *response* until the
    inbound stream reaches EndStream/validated) is larger and would
    diverge from the existing collect()-based sibling paths. I propose
    the buffer-then-forward approach (consistent, deterministic,
    bounded) and will proceed with it unless lead directs otherwise in
    the approval.

### Fix (b) — reject pseudo-header in trailers, both protocols
- H2: at BOTH trailer-capture sites (h2_proxy.rs:1168 and :1386), after
  building `trailers_vec`, if ANY trailer name `starts_with(':')`
  return `Err("pseudo-header field in trailers (RFC 9113 §8.1)")` —
  the existing `?`/`map_err` plumbing turns that into an error response
  / connection failure (PROTOCOL_ERROR-class), never a forwarded body.
  Mirror the `h2_to_h2.rs:19` filter intent but as a REJECTION (RFC
  mandates PROTOCOL_ERROR, not silent strip, for the trailing field
  section).
- H3: in `feed_body` `InTrailers` arm (h3_bridge.rs:382-388), after
  `decoder.decode(&block)`, if any decoded trailer name
  `starts_with(':')` return `Err("h3 trailer pseudo-header (RFC 9114
  §4.3)")`. The existing `feed_body` `Err` contract in
  `conn_actor.rs:692-700` already maps this to `ReqBodyEvent::Reset` +
  stream teardown (PROTOCOL_ERROR-class) — exactly the mandated
  rejection, no new plumbing.

### Regression test
- New `tests/h2_validation_before_forward_under_load.rs`: runs the
  installed `h2spec` binary (confirmed on PATH at
  `/home/ubuntu/.cargo/bin/h2spec`) against the real H2s listener while
  the SAME in-test contention harness from F-SEC-1 induces
  concurrent-runtime starvation; asserts h2spec exit 0 (0 failures)
  across the 5 implicated cases. Pre-fix: capture a failing run (one of
  the 5 faces shows `Actual: DATA Frame (length:2 ...)`); post-fix:
  green ×3 under churn. Graceful skip ONLY if h2spec disappears from
  PATH (it is present now — will run for real).
- Targeted unit tests for rule (b):
  - lb-l7: a unit test feeding a request with a `:`-prefixed trailer
    through each capture site asserting `Err` (no forward).
  - lb-quic: a `feed_body` unit test with a QPACK-encoded trailer
    block containing a pseudo-header asserting `feed_body` returns
    `Err` (drives the conn_actor Reset path). Plus an existing-valid-
    trailer test still passes (no regression — guard the
    h3_h1_trailers_resp_e2e contract).

### Risks
- Buffering request body: bounded by the cap above; behavior now
  matches sibling H2→H2/H2→H3 paths. `trailer_passthrough.rs`,
  `h3_h1_trailers_resp_e2e`, `bridging_*` must stay green (valid
  trailers still forwarded; only pseudo-prefixed ones rejected) — I
  run these targeted suites post-fix.
- h2spec under churn can be slow; bounded h2spec `--timeout` + bounded
  harness rounds; targeted (not workspace) run.

---

## Finding 3 — F-COR-6 (CORRECTNESS) — stale-false S2 #[ignore]

### Root cause (confirmed, matches auditor-4 F-2)
`crates/lb-quic/src/h3_bridge.rs:1656-1677`
`s2_target_build_h1_request_with_body_sets_content_length_and_appends_
payload` is `#[ignore = "S2: request-body forwarding"]` with a doc
comment (:1645-1655) claiming the datapath is UNBUILT. FALSE: S2 P1-A
built `poll_h3` DATA accumulation (e2e-proven, auditor-4 cites green
t1/t5/non_utf8). `build_h1_request(&req, Some(b"hello-s2-body"))`
already produces exactly the asserted bytes.

### Fix
- Remove `#[ignore = "S2: request-body forwarding"]` (line 1657).
- Rewrite the doc comment (:1645-1655) to state the datapath is BUILT
  (S2 P1-A), citing the e2e proofs (t1 multi-DATA-frame binary body,
  t5 memory-bounded large DATA, h3_to_h1_forwards_non_utf8_body), and
  drop the false "UNBUILT / no caller passes Some" sentence.
- No assertion change (already passes — confirmed by reading
  `build_h1_request` at h3_bridge.rs:583-615).

### Regression / verification
`cargo test -p lb-quic --lib` (and the relevant
`s2_target_build_h1_request_*` filter) shows the test `... ok` and
ignored count for that test drops to 0. This is finding 3; same file
as F-COR-1's h3_bridge change so it is done AFTER F-COR-1 and committed
separately.

---

## Order & commits
1. F-SEC-1: edit h2_proxy.rs:515 arm + new reg test → build + new test
   (pre-fix fail captured, post-fix green ×3) + targeted suites
   (h2_security_live, h2_proxy_e2e) → commit → checkpoint lead.
2. F-COR-1: ordering fix in proxy_request + pseudo-trailer rejection
   (h2_proxy.rs:1168/:1386, h3_bridge.rs:382-388) + reg tests → build +
   h2spec-under-churn + unit tests + targeted trailer/bridging suites →
   commit → checkpoint lead.
3. F-COR-6: un-ignore + doc fix → `cargo test -p lb-quic` filter green
   → commit → checkpoint lead.

Each commit:
`git -c user.email=build@local -c user.name=builder-1 commit`. No push.
TaskUpdate unavailable in env — I will note task #7/#8/#9 state
transitions in `messages/builder-1-to-lead.md` (in_progress on start,
completed when my part done) and ask lead to mirror into the task
system if that is the canonical channel.

AWAITING "approved" in messages/lead-to-builder-1.md before any source
edit.
