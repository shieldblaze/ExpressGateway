# F-SEC-1 v2 — Rapid-Reset GOAWAY must reach the wire (CVE-2023-44487)

Builder: builder-1. Branch: audit/foundation-pass. Tier: SECURITY (R6 unconditional, no asterisk).

## Root cause (proven from hyper-1.9.0 + h2-0.4.13 source + phase3-final R1 evidence)

Rapid-reset enforcement is delegated to hyper/h2 via
`H2SecurityThresholds::apply()` (`max_pending_accept_reset_streams` /
`max_local_error_reset_streams`). Trace on the trip:

1. `h2::server::Connection::poll_accept` → `poll_closed` → `connection.poll()`.
2. `poll2` → `recv_frame` trips the local-error-reset counter →
   `Err(Error::GoAway(ENHANCE_YOUR_CALM, Library))`.
3. `handle_poll2_result` → `handle_go_away`: queues the GOAWAY frame
   (`go_away_now_data`) and sets `State::Closing`.
4. Next loop iter: `State::Closing` → `ready!(self.codec.shutdown(cx))?`
   = `framed_write::shutdown` = `flush()` (writes the GOAWAY bytes via
   `poll_write_buf`+`poll_flush` into the inner io) **then**
   `inner.poll_shutdown(cx)` (the FIN).
5. h2 transitions `State::Closed`, `connection.poll()` returns
   `Poll::Ready(Err(library_go_away))`.
6. hyper `proto/h2/server.rs:314` `Some(Err(e)) => return
   Poll::Ready(Err(...))`. Our select arm `res = &mut conn` (h2_proxy.rs
   ~669) returns `Err`, **drops `conn` → drops the io**.

So h2 *does* push the GOAWAY into the kernel TCP send buffer before the
FIN. The defect is the **abortive RST close**: the prior `CleanCloseIo`
drains inbound only until the first `Poll::Pending`, then **breaks and
FINs anyway**. During a rapid-reset flood the abusive client is
*continuously* streaming RST_STREAM frames, so the server kernel recv
buffer is essentially never durably empty and, even when momentarily
empty, the client still has bytes in flight. Closing/dropping a TCP
socket while the peer is still actively sending makes Linux emit an
**RST** instead of a clean FIN; the client's TCP stack discards its
entire receive buffer — including the already-arrived GOAWAY — on the
RST, surfacing only `Io(BrokenPipe)` with `send_err=None`. This is the
exact phase3-final signature (`conn_res=Ok(Ok(Err(Io(BrokenPipe))))`,
neither `is_go_away()` nor `is_remote()`), nondeterministic (~1/3)
because whether the close races to RST depends on scheduler timing of
the client's read task vs. the server FIN under 8-core contention.

The prior unit proxy `clean_close_io_drains_inbound_before_fin` passed
only because it modelled "finite data then clean EOF" — it never
modelled the real condition (peer keeps sending past the FIN), which is
exactly why it false-verified.

## Fix (server-side, structural, RFC-correct lingering close)

Replace the break-on-`Pending` drain with a **proper graceful
lingering close** in `CleanCloseIo::poll_shutdown` (the standard
nginx-`lingering_close`/`SO_LINGER`-avoidance pattern):

- The GOAWAY has already been flushed by h2's `codec.shutdown` flush
  (step 4) BEFORE `poll_shutdown` is invoked — keep relying on that
  (verified in source; do not change h2/hyper).
- In `poll_shutdown`, drain (read+discard) inbound until **EOF** (peer
  saw the GOAWAY and closed its write half — the normal well-behaved
  reaction) bounded by BOTH a byte cap AND a wall-clock deadline.
- On `Poll::Pending` during the drain, **return `Poll::Pending`**
  (register the waker, yield) instead of breaking to FIN — so we
  actually wait for the peer's post-GOAWAY FIN rather than racing it
  with our own RST-causing close.
- The wall-clock deadline (a `tokio::time::Sleep`, bounded, named
  const) guarantees a deliberately-wedged/silent client cannot pin the
  worker: once it elapses we stop draining and proceed to the inner
  `poll_shutdown` regardless. The byte cap remains as a second bound.
- Only after EOF or the deadline do we delegate `inner.poll_shutdown`.

Net effect: the client receives `... GOAWAY ... FIN` in order on a
cleanly half-then-fully-closed socket (no RST), so `h2::client`
decodes the GOAWAY and the conn future resolves
`Err(GoAway(_, ENHANCE_YOUR_CALM|PROTOCOL_ERROR, Remote))` →
`is_go_away()` / `is_remote()` true → the test's
`server_initiated` assertion holds. DoS mitigation is unchanged
(connection still dies, bounded). Entirely server-side; no protocol
behaviour change for conformant peers.

## Proof (acceptance — non-negotiable, proxy-proof)

- Pre-fix: phase3-final/RESULT.md captured verbatim FAIL under the real
  R1 condition (cited) + local reproduce attempt.
- Post-fix GATE = the REAL wire test
  `tests/h2_security_live.rs::rapid_reset_goaway` GREEN for **≥15
  consecutive runs each executed UNDER full
  `cargo test --workspace --all-features -- --test-threads=8` 8-core
  contention**, zero `BrokenPipe`/non-GOAWAY teardowns, plus the R1
  triple `--all-features` ×3 all-green.
- Keep the prior unit/corroboration tests as ADDITIONAL coverage
  (not weakened, not deleted — R5). They are not the gate.
- Do NOT modify the `rapid_reset_goaway` assertion. The server must
  actually send+land the GOAWAY.

No R7 product-behaviour fork: lingering-close is the unambiguous
RFC-correct behaviour; this is pre-authorized SECURITY scope.
Proceeding without waiting for lead approval per directive.
