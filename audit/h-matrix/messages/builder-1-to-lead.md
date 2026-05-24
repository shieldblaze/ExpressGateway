# builder-1 → lead (append-only log) — S9 H1→H1 cell

ENV NOTE: no SendMessage tool in my environment. Coordinating via these
files (mirrors the foundation-pass convention). Please reply by writing
`audit/h-matrix/messages/lead-to-builder-1.md` with "approved" (or
change requests). Per R5 I will NOT touch source until that file says
approved.

Worktree `/home/ubuntu/Code/s9-builder1`, branch `s9/builder-1`, base
`a2e0f6fe`. `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`. Disk 27 GB
free at start (above the 25 GB floor).

---

## 2026-05-23 — I1+I2 DESIGN SKETCH (awaiting approval)

Spec read: `s9-h1h1-plan-RESOLVED.md`, `s8-h2h1-verify.md`, the M-D
reference (`h2_proxy.rs` proxy_request + PumpAbort + record_retained),
the target (`h1_proxy.rs::proxy_request` @1073-1127), the regression
locks (`round8_body_overread.rs`, `trailer_passthrough.rs`,
`h1_proxy_e2e.rs`, smuggle tests).

### Q-H3 DECODER FINDING (the gating fact for the trailer guard)

Resolved hyper = **1.9.0** (Cargo.lock). I read its inbound H1 chunked
trailer decoder `decode_trailers`
(`.../hyper-1.9.0/src/proto/h1/decode.rs:651`). It only validates
header-NAME and header-VALUE *syntax* and `trailers.insert(name,value)`
for every field — it does **NOT** strip or reject the forbidden
framing/routing fields (Transfer-Encoding, Content-Length, Host,
Trailer, TE, Connection). So per Q-H3 branch (b): the live decoder does
NOT enforce it → I MUST add a minimal forbidden-framing-trailer → 400
guard, with a real-wire test. (I will NOT reuse h2's
`validate_request_trailers` — that one rejects H2 pseudo-headers `:`,
which is the wrong check for H1; I add an H1-appropriate forbidden-name
check.)

### F-MD-4 H1 DIVERGENCE FROM H2 (important — please sanity-check)

I read hyper-1.9.0's inbound body `poll_frame`/`is_end_stream`
(`body/incoming.rs:193,282`). The H1 server inbound body is `Kind::Chan`
(channel fed by the conn driver), NOT `Kind::H2`. Consequences:
- A premature client TCP half-close mid-body surfaces as the decoder's
  `IncompleteBody` (UnexpectedEof) pushed into the body channel →
  `body.frame()` yields **`Some(Err(..))`**, NOT a clean `None`. (decode.rs
  :162/:504 produce IncompleteBody on early EOF for both chunked and
  content-length bodies.)
- `is_end_stream()` for `Kind::Chan` is `content_length==ZERO` — it is
  NOT a reliable post-hoc "clean end" signal the way H2's is (chunked is
  CHUNKED/CLOSE_DELIMITED, never decremented to ZERO). So I do NOT use
  is_end_stream for H1.
- Therefore for H1 the positively-confirmed clean end is **`frame()==None`**
  (the decoder reached the real chunked `0\r\n\r\n` / satisfied CL and
  dropped the sender). A truncation is the distinguishable `Some(Err)`.

So H1 F-MD-4 is the MIRROR-IMAGE of H2's: H2 had to reject ambiguous
`None`; H1's `None` IS the clean signal and `Some(Err)` is the abort
signal. I will document this divergence verbatim at the pump site (it is
the single most likely place a future reader mis-copies the H2 logic).
Net rule, identical safety property: the upstream chunked terminator is
written ONLY on `None` (clean); on `Some(Err)` I inject `PumpAbort` so
hyper aborts the upstream request WITHOUT a terminator (truncated request
never seen complete upstream). Single-use `take_stream` already prevents
pooling the aborted upstream.

### Branch-B-only (no lookahead)

H1 ingress has no HPACK / no H2 framing / no validate-before-forward
ordering requirement, so per the plan I forward-as-it-arrives
immediately — NO lookahead buffer, NO zero-dial regime. I dial the
upstream first (as the code does today), then run the pump. This is
strictly the H2 "Branch B" shape with the lookahead machinery removed.

### Pump shape (types / channel / StreamBody / decrement site)

In `proxy_request`, AFTER `take_stream()` (kept verbatim, single-use) and
the handshake, instead of `sender.send_request(req)` on the raw body:

1. Split inbound: `let (mut parts, mut body) = req.into_parts();`
2. **F-MD-1**: `parts.version = HTTP_11`; `parts.headers.remove(CONTENT_LENGTH)`;
   `.remove(TRANSFER_ENCODING)` — so hyper frames the unknown-length
   StreamBody as chunked itself (a stale inbound CL + StreamBody mis-frames
   identically to F-MD-1).
3. Handshake typed `http1::handshake::<_, BoxBody<Bytes, PumpAbort>>` so
   the body error type is the constructible `PumpAbort` (imported from
   h2_proxy — see note below).
4. Bounded channel: `mpsc::channel::<Result<Frame<Bytes>, PumpAbort>>(DEPTH)`
   DEPTH=8, chunk max=8 KiB → fixed 64 KiB in-flight window (mirrors the
   M-D window; body-size-INDEPENDENT and independent of the 64 MiB cap).
5. `in_flight_bytes: Arc<AtomicUsize>`; `StreamBody::new(stream::poll_fn(...))`
   that, on each `Ready(Some(Ok(frame)))` with a `data_ref()`, does
   `in_flight_body.fetch_sub(len)` — the DECREMENT site (the moment hyper
   pulls the chunk). `.boxed()` → BoxBody<Bytes,PumpAbort>.
   `Request::from_parts(parts, stream_body)`.
6. `oneshot` verdict channel `Result<(),ProxyErr>` for the
   validate-before-RESPONSE-relay gate.
7. Spawn the pump: loop `body.frame().await`:
   - `None` → clean EOF: drop `tx` (verdict Ok). hyper writes terminator.
   - `Some(Ok(frame))` data → `forwarded_total += len`; **Q-H4 cap**: if
     `> MAX_REQUEST_BODY_BYTES` (imported, 64 MiB) → send `Err(PumpAbort)`,
     verdict `Err(BodyTooLarge)`. Else `send_chunked!` (split to 8 KiB,
     `fetch_add` + `record_retained(in_flight)` before each push). If the
     channel send fails → **F-MD-2** ReceiverGone → drain-and-validate
     (discard bytes, still apply cap + trailer guard, relay backend resp);
     NOT a 413.
   - `Some(Ok(frame))` trailers → **Q-H3** `validate_h1_request_trailers`
     (forbidden-framing-name → BadRequest). Ok → forward the trailers
     Frame through the channel, verdict Ok. Err → `Err(PumpAbort)` +
     verdict Err.
   - `Some(Err(e))` → **F-MD-4** truncation: `send(Err(PumpAbort))`,
     verdict `Err(BadRequest("inbound H1 request body incomplete: {e}"))`.
8. Drive `sender.send_request(req)` under `timeouts.body` concurrently
   with the pump (hyper must pull for the pump to progress under
   backpressure). On send Ok, gate on `verdict_rx.await`: Ok→relay resp;
   Err→abort conn_handle + pump, return the ProxyErr (never relay).
   On send Err/timeout: abort pump+conn, return Upstream/Timeout.

### F-MD-3 gauge (I2)

New `#[cfg(any(test, feature="test-gauges"))]` static
`H1_REQ_MAX_RETAINED_BODY_BYTES: AtomicUsize` + `record_retained_h1(n)`
(lock-free CAS-max), defined in h1_proxy.rs (NOT reusing the H2 symbol —
distinct gauge name the plan names explicitly). Records
`in_flight_bytes` live occupancy at each push (decrement in the poll_fn).
No lookahead term for H1 (no lookahead buffer), so the retained set IS
the in-flight channel occupancy ≤ 64 KiB. Non-vacuous: a whole-body
buffer variant would push it over the window ceiling. Increment-on-push /
decrement-on-pull, never a constant.

### ProxyErr — NEW VARIANTS NEEDED (flagging)

The H1 `ProxyErr` (h1_proxy.rs:1361) currently has ONLY `Upstream` and
`Timeout`, and the call site (:1024-1030) maps only those. I must add
`BadRequest(String)` → 400 and `BodyTooLarge` → 413, and extend the
match arm at :1024 to map them (mirrors h2_proxy's ProxyErr + the
413/400 responses). This is the minimal surface change required by
Q-H3/Q-H4/F-MD-2/F-MD-4. Flagging since it touches the enum + call site.

### PumpAbort + MAX_REQUEST_BODY_BYTES reuse

Per task §3 I will IMPORT and reuse `MAX_REQUEST_BODY_BYTES` from
h2_proxy (it is `pub`), NOT redefine it. `PumpAbort` is currently a
private `struct` in h2_proxy — I will make it `pub(crate)` (a 1-word
visibility change, no logic touch to the BUILT M-D cell; it does not
alter behavior or re-open M-D verification) and import it, rather than
defining a second identical type. If you prefer I define a separate
`H1PumpAbort` to avoid ANY edit to h2_proxy.rs, say so — I will default
to the `pub(crate)` reuse unless you object.

### Constants

I will add H1-named consts `H1_REQ_CHANNEL_DEPTH = 8` and
`H1_REQ_CHUNK_MAX = 8*1024` in h1_proxy.rs (same 64 KiB window) rather
than reuse the H2-named consts in the H1 path (clarity); window value is
identical to M-D as R8 requires.

### Increment plan

- **I1**: pump + StreamBody + F-MD-1 + F-MD-4 + Q-H3 guard wiring +
  single-use intact + ProxyErr variants/mapping. (Trailers forwarding is
  intrinsic to the pump so it lands here; the gauge + explicit cap test
  surface land in I2.) Compile `--all-features`, run regression set
  (lib tests, h1_proxy_e2e, bridging_h1_h1, round8_body_overread,
  smuggle_*), commit, push.
- **I2**: `H1_REQ_MAX_RETAINED_BODY_BYTES` gauge + record site, confirm
  64 MiB cap path, F-MD-2 drain-and-validate finalize. fmt + clippy.
  commit, push.

(I3 real-wire/memory/backpressure/coverage proofs are the independent
verifier's, not mine.)

### Regression-lock preservation

- ROUND8-L7-10 doc-block (:1046-1072) and `take_stream()` single-use
  (:1093) UNCHANGED — `round8_body_overread.rs` stays green.
- Trailers forwarded faithfully → `trailer_passthrough.rs` /
  `bridging_h1_h1.rs` stay green.
- Pre-pump header-level CL/TE smuggle rejection (`SmuggleDetector::
  check_all_mode` @ :961) is untouched and still fires before
  proxy_request — smuggle tests stay green.

Standing by for approval / corrections before any source edit.

---

## 2026-05-23 — I1 + I2 DONE (built per approval; awaiting independent verifier)

Implemented exactly to the approval (MOD1 local `H1PumpAbort`, h2_proxy.rs
BYTE-UNCHANGED; MOD2 both CL + chunked framings safe; both reaffirmations
explicit in source).

**I1 — bounded ingress pump + StreamBody — SHA `b01a13d2`**
- Branch-B-only bounded pump in `H1Proxy::proxy_request` (mirror of the M-D
  pump, lookahead removed): bounded mpsc depth 8 x 8 KiB = 64 KiB fixed
  in-flight window -> in-flight-counting `StreamBody` -> hyper http1 sender.
  No whole-body buffer, no collect(), no Limited.
- F-MD-1: parts.version=HTTP/1.1 + strip CL/TE.
- F-MD-4 (H1 MIRROR-IMAGE, documented verbatim at the pump site): `None` =
  positively-confirmed clean end; `Some(Err)` (IncompleteBody on early EOF,
  BOTH CL and chunked) = truncation -> inject `H1PumpAbort` BEFORE the verdict
  (FIFO Err-before-close) -> hyper aborts upstream WITHOUT terminator. Do NOT
  use is_end_stream (unreliable for Kind::Chan). On Err verdict the caller
  aborts conn_handle+pump and returns ProxyErr, never relays the response;
  take_stream() single-use kept (aborted upstream dropped, not pooled).
- Q-H4: imported `MAX_REQUEST_BODY_BYTES` (NOT redefined) 64 MiB -> 413.
- F-MD-2: receiver-drop -> drain-and-validate (relay backend early response),
  NOT a 413.
- Q-H3 branch (b): `validate_h1_request_trailers` rejects forbidden framing/
  routing trailer fields (CL/TE/Host/Trailer/TE/Connection + HOP_BY_HOP) ->
  400; legitimate trailers forwarded byte-faithfully. (Decoder fact: hyper-1.9.0
  `decode_trailers` rejects none — confirmed in source.)
- ProxyErr += BadRequest(String)->400, BodyTooLarge->413; call-site + WS-dial
  match arms updated.
- Q-H2 preserved: ROUND8-L7-10 doc-block + take_stream() single-use intact.

**I2 — non-vacuous gauge (F-MD-3) — SHA `0a479579`**
- `pub static H1_REQ_MAX_RETAINED_BODY_BYTES` + `pub fn record_retained_h1`
  (CAS-max) behind `#[cfg(any(test, feature="test-gauges"))]`, distinct symbol
  from the H2 gauge, reachable as
  `lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES` under --all-features (R10).
- Record site in the pump's `send_chunked!`: records LIVE in_flight_bytes on
  each push; decrement in the StreamBody poll when hyper pulls. Non-vacuous.

**Checks I ran (author-side; the real proofs are the verifier's):**
- `cargo build -p lb-l7 --all-features`: clean. Also default-features (gauge
  off) clean.
- `cargo clippy -p lb-l7 --all-targets --all-features -- -D warnings`: clean.
  Also default-features clippy clean.
- `cargo fmt --check -p lb-l7`: clean.
- Regression (all green): lb-l7 lib 91; full lb-l7 integration suite (all 31
  test binaries ok); h1_proxy_e2e 3/3; bridging_h1_h1 1; round8_body_overread
  4 (doc-block provenance green); trailer_passthrough 8; smuggle_matrix 13;
  smuggle_wired 3; round8_keepalive_count_cap 3; security_smuggling_{cl_te,
  te_cl,h2_downgrade} 3.
- h2_proxy.rs untouched (MOD1): `git diff a2e0f6fe -- crates/lb-l7/src/h2_proxy.rs`
  is empty.
- Disk: 27 GB free (above the 25 GB floor) throughout.

**NOT self-verified (verifier owns, per the bar):** real-wire both-direction
binary byte-identity; non-vacuous/body-size-independent memory + inverted
probe; backpressure causal chains; F-MD-4 premature-close complete=0 on the
wire for BOTH CL and chunked; 64 MiB->413 real-wire; >=80% coverage sub-metric.
I did NOT self-declare BUILT.

Pushed to origin/s9/builder-1 (tip `0a479579`). Standing by for verifier
findings / any lead corrections.

---

## 2026-05-24 — F-CAP-1 DONE (both cells; owner-approved fix) — SHA `23b45d6f`

Applied the approved caller-side fix IDENTICALLY in both cells; did NOT
self-declare BUILT (verifier re-verifies both).

**The fix (one combined commit):** on the `send_fut` `Ok(Err(e))` arm, do NOT
`pump.abort()` first — await `verdict_rx` BOUNDED by `self.timeouts.body`. If
the verdict is `Ok(Err(BodyTooLarge | BadRequest(..)))` return THAT (413/400);
otherwise (verdict Ok, non-classified error, pump vanished, or the bounded
await elapses) fall through to `Upstream(502)` / Timeout-on-elapse. Then abort
pump + conn_handle. Deterministic per your reasoning: deliberate abort -> pump
sends verdict immediately after the body Err (FIFO) -> bounded await resolves
classified; genuine upstream failure -> ReceiverGone -> drain-and-validate ->
Ok/non-classified verdict -> 502.

- `crates/lb-l7/src/h1_proxy.rs` (H1->H1, my cell).
- `crates/lb-l7/src/h2_proxy.rs` (H2->H1 shipped M-D cell): `git diff a2e0f6fe`
  is EXACTLY and ONLY the caller-side arm (pasted shape: classified-verdict
  match + unwrap_or_else fallback). Everything else byte-for-byte unchanged.
  PRESERVED: FIFO Err-before-close, is_end_stream gating (H2) / None=clean-end
  (H1), single-use take_stream.

**No 502-leak / no spurious-413 audit (your two required checks):**
- Over-cap and forbidden-trailer never yield 502: the pump injects
  H1PumpAbort/PumpAbort then sends the classified verdict FIFO; the bounded
  verdict await (10–20 s in tests, `timeouts.body` in prod) resolves with
  BodyTooLarge/BadRequest before elapse -> 413/400. PROVEN by the
  NEGATIVE CONTROL: reverting to the pre-fix unconditional-502 arm makes
  `over_cap_content_length_upload_yields_413_not_502` fail `left:502 right:413`.
- Genuine upstream failure never yields 413/400: a backend that drops the
  conn (no HTTP) -> ReceiverGone -> drain-and-validate of a within-cap,
  trailer-clean body -> verdict Ok(()) (non-classified) -> None -> 502.
  PROVEN by `genuine_upstream_failure_still_yields_502` (H1) and
  `h2_genuine_upstream_failure_still_yields_502` (H2).

**413/400 evidence (CLIENT-observed status, deterministic 3x; no R2 race):**
- H1 `over_cap_content_length_upload_yields_413_not_502` -> 413 (raw client
  reads the status line; >64 MiB Content-Length upload, backend draining).
- H1 `over_cap_chunked_upload_yields_413_not_502` -> 413 (chunked framing).
- H1 `forbidden_framing_trailer_yields_400_not_502` -> 400 (chunked body +
  `Transfer-Encoding` smuggled in the trailer section).
- H1 `genuine_upstream_failure_still_yields_502` -> 502.
- H2 `h2_over_cap_upload_yields_413_not_502` -> 413 (reqwest H2, >64 MiB).
- H2 `h2_genuine_upstream_failure_still_yields_502` -> 502.
All 6 stable across 3 repeated runs each.

**Regression (all green):** h1_proxy_e2e 7; h2_proxy_e2e 5; h2h1_md_streaming_
verify 14; h2h1_md_coverage_driver 11; bridging_h1_h1 1; bridging_h2_h1 1;
round8_body_overread 4; round8_keepalive_count_cap 3; smuggle_matrix 13;
smuggle_wired 3; trailer_passthrough 8; lb-l7 lib 91. `cargo fmt --all`
clean; `cargo clippy --all-targets --all-features -D warnings` clean. Disk
46 GB free.

Pushed to origin/s9/builder-1 tip `23b45d6f`. NOT self-declaring BUILT —
verifier re-verifies BOTH cells. Standing by.
