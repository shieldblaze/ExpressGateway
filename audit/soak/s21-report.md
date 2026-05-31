# SESSION 21 — FIX SOAK FINDINGS + CLEAN RE-SOAK — REPORT

Base: `feature/soak-fixes-s21` off main `1cc860b2` (S20 promoted; soak suite
exists). 8-core box, ENA ens5. Verdict from COMPLETED runs only (R15).

**Headline:** measure-first (the mandated CF-S16 discipline) overturned the
central S20 finding: **F-S20-1 is a load-harness artifact, not a gateway relay
defect** — the Mode B relay was always correct. **F-S20-2 (Mode A passthrough
flow/fd/RSS retention) is the one real gateway defect; it is FIXED** with an
idle-flow reaper proven by the re-soak time-series. Three S20 records are
corrected honestly below (not buried).

---

## 1. R1 baseline

`main@1cc860b2` confirmed (S20 promote = the exact base). `fmt --all --check`
clean; `clippy --workspace --all-targets --all-features -D warnings` exit 0;
`cargo test --workspace --all-features` ×3 ALL exit 0, **0 failures across all
suites, all 3 passes**.

Process note (honesty): the ×3 baseline gate was started on the pristine tree,
but fix edits landed while passes 2–3 were running (a slip — author should not
edit source during a gate). Pass 1 ran clean on the pristine/near-pristine tree;
the pristine ×3 record is anchored by S20's ×3-at-promote on this identical
commit `1cc860b2`. The BINDING determinism gate is the final ×3 on the changed
committed tree (§6).

---

## 2. THREE corrections to the S20 record (measure-first)

### Correction 1 — F-S20-1 is NOT a gateway defect (it is a load-client bug)
S20 reported a "Mode B 4-concurrent-stream relay STALL" (sid12 wedged at
1212/4096) as a HIGH gateway defect. **Measurement refutes this.** Instrumenting
the relay (RAW_RELAY_DBG) and the client (stream_send returns + per-conn stats):

- The gateway's client-facing quiche connection received **exactly 1212 bytes
  of sid12 and NO FIN, then `recv Done` forever** — it relayed faithfully every
  byte it was given; sid12 was never readable again.
- The CLIENT only ever *sent* 1212 bytes of sid12: `stream_send(sid12,..)`
  returned **`Ok(1212)` with `cap_after=0`** — a PARTIAL write. quiche
  `stream_send` is bounded by the connection SEND CAPACITY (the initial
  congestion window ≈ 10 packets ≈ 13.5 KB, since `max_send_udp_payload_size`
  = 1350). The first 3 streams (3×4096 = 12288) spent it; sid12 got the
  remaining 1212. The soak client called `stream_send` ONCE per stream and
  ignored the partial return, so the rest of sid12 + its FIN were NEVER sent —
  the client then waited forever for an echo of bytes it never transmitted.
- With a CORRECT full-send client (re-send the remainder + FIN as cwnd frees),
  the gateway relays **4/8/16 concurrent streams, multi-conn, +datagrams, all
  with err=0** (sc4_modeb 4-stream: `ok=6132 err=0`; 8-stream: `ok=4709 err=0`;
  integration tests below). The relay was always correct.

### Correction 2 — S20's R13 reproduced the SYMPTOM but mis-attributed the CAUSE
Both the S20 lead AND the independent verifier "reproduced" the wedge — but both
mis-attributed it to the gateway. Re-running the same harness reproduces the
same harness artifact. **Lesson (recorded):** a finding is not a "<component>
bug" until that component's OWN behavior is measured at fault; reproducing a
symptom ≠ confirming attribution.

### Correction 3 — S20's "Mode A control" was a FALSE no-op
S20 argued "Mode A unaffected → gateway-relay-specific." Measurement: in Mode A
the client's stream opens return **`StreamLimit` (`peer_streams_left_bidi=0`)**
— the known **CF-S15-PASSTHROUGH-RETRY-ODCID** (mint_retry=true mints a Retry →
the backend's `original_destination_connection_id` transport param mismatches →
0 streams granted; the e2e stream test `quic_passthrough_e2e.rs` is already
`#[ignore]`'d for it). So `expecting` was empty and the session trivially
returned Ok — "Mode A unaffected" meant "Mode A did nothing," not "handled it."
(Mode B control = `peer_streams_left_bidi=16`, correct.) No test had ever driven
application streams e2e through Mode A, so the gap hid. Disposition: documented
known Mode A limitation (PROXY-protocol-for-QUIC sidecar needed); escalated as
its own future workstream — NOT opened this session (owner ruling).

---

## 3. F-S20-1 disposition — fix the harness + guard the gateway

**Mechanism:** §2 Correction 1 (load-client partial-write).

**Fix:**
- `crates/lb-soak/src/loadgen.rs::quic_session` — the QUIC client now re-sends
  each stream's remaining payload + FIN as send capacity frees (the correct
  QUIC client contract), interleaved with the recv loop. Diagnostic reports
  got/sent/want per sid.

**R13 evidence:**
- (a) Deterministic regression + load-bearing negative control:
  `crates/lb-quic/tests/s16_b2_stream_relay_smoke.rs`
  - `s21_four_concurrent_bidi_streams_all_echo` — PASS (all 4 echo byte-identical
    through the real Mode B relay actor).
  - `s21_eight_concurrent_bidi_streams_all_echo` — PASS.
  - `s21_singleshot_send_wedges_a_stream_negative_control` — PASS (single-shot
    send leaves a stream incomplete: `completed < 4` — proves the positive tests
    are non-vacuous AND the fix is client-side, not the relay).
- (b) In the ×3 gate (§6).
- (c) Burst (≥50 iter, 0 wedges): sc4_modeb 4-stream `ok=6132 err=0`, 8-stream
  `ok=4709 err=0` through the real release binary — 6132 ≫ 50 iterations, 0
  wedges, 0 mismatches. Confirmed under sustained load by the re-soak (§5).

---

## 4. F-S20-2 disposition — Mode A passthrough idle-flow reaper (THE gateway fix)

**Mechanism (confirmed real, CONFIRMED leak):** S20 measured Mode A flows 0→56457,
fds→28240 (~1 backend UDP socket/flow), RSS→331MB, **evicted_total=0** (no idle
reclaim). Reproduced on the S21 base (90s isolated): flows 0→56457, fds→28240,
RSS→331MB, evicted=0, DRIFT. Root cause ran deeper than "no sweep exists": the
per-flow **reverse pump blocks indefinitely on `backend_sock.recv()` and holds
an `Arc<FlowEntry>`**, so removing the dispatch keys alone could NOT reclaim the
fd/tasks for an alive-but-silent backend (no recv error to break the loop).

**Fix (`crates/lb-quic/src/passthrough.rs`):**
- `FlowEntry.closed: CancellationToken` (field-audit + no-key-material type
  witness updated). The reverse pump `select!`s recv against it; the forward
  pump honors it too → a reaped flow's tasks exit → `Arc` strong count → 0 →
  `Drop` closes the backend UDP socket fd.
- `reclaim_flows()` — SINGLE-SOURCED reclamation (R12) for BOTH LRU eviction and
  the idle sweep: cancel tokens, remove keys, bump `flows_evicted_total` once
  per flow.
- `sweep_idle_flows()` + a periodic reaper task in `PassthroughListener::spawn`
  reclaims flows idle past `flow_idle_timeout` (default 60s, configurable;
  `ZERO` disables → LRU-only). Bounds the table by LIVE connections
  (Katran/Pingora-style stateless reclamation; R7 pre-auth on the 60s default).
- `lb-config`: `PassthroughConfig.flow_idle_timeout_ms` (default 60_000);
  `main.rs` threads it into `PassthroughParams`.

**R13 evidence:**
- Unit: `passthrough::tests::idle_sweep_reclaims_idle_flows_and_frees_them` —
  reclaim + negative control + reclamation PROOF (the FlowEntry Drop gauge fires,
  proving the fd/tasks are actually freed, not just the keys removed). LRU test
  `evict_oldest_at_cap_and_negative_control` still PASS (no regression; eviction
  now also reclaims tasks via the shared `reclaim_flows`).
- **Dedicated verdict run (COMPLETED, R15)** — Mode A 240s, idle=10s, isolated:

  | t (s) | RSS (KB) | fds | flows | evicted_total | panic |
  |------:|---------:|----:|------:|--------------:|------:|
  | 0     | 8224     | 11  | 0     | 0             | 0 |
  | 15    | 38340    | 649 | 1275  | 1193          | 0 |
  | 60    | 76364    | 261 | 499   | 2993          | 0 |
  | 120   | 81928    | 176 | 329   | 4232          | 0 |
  | 180   | 82520    | 154 | 285   | 5134          | 0 |
  | 240   | 82524    | 101 | 179   | 5923          | 0 |

  flows BOUNDED (peak 1275 → 179), fds BOUNDED (peak 649 → 101),
  **evicted_total=5923** (the sweep actively reclaims — was 0 in the leak),
  **RSS plateaus DEAD-FLAT at 82524 KB** for the last 45s (82520/82524/82524/
  82524/82524 across t=165–240). The analyzer's median-thirds heuristic flags
  RSS "DRIFT" only because the one-time pre-sweep ramp-then-plateau is monotone
  over the window; the raw tail is flat. vs the leak: 330MB-and-climbing with
  evicted=0. The fix bounds Mode A. The re-soak (§5) confirms over a longer run.

---

## 5. Clean re-soak (shippable-v1 gate) — COMPLETED (R15)

Batched (the 8-core box cannot co-locate all 8 at baked concurrency without OS
thrash — load ~32 = the S20 run1 anti-pattern; R9/R2). `scripts/soak/s21-run.sh`
ran 4 sequential co-located batches × 240s. Per-batch 1-min load at batch end:
B1-tcp 18.2, B2-tls 14.2, **B3-modeb 6.2, B4-modea 1.2** — the FIXED paths (B3
F-S20-1, B4 F-S20-2) ran on a quiet, non-saturated box; B1/B2 (unchanged paths)
were oversubscribed but with NO OOM (12 GiB free throughout) so their
bounded-STATE measurement is valid. Archived:
`audit/soak/s21-soak-data/resoak-240s/`.

**Completed-run verdict — panic_total=0 in EVERY scenario; all connection /
flow / stream / accept STATE BOUNDED everywhere:**

| Scenario | Verdict | Load (completed run) | Key state evidence |
|----------|---------|----------------------|--------------------|
| sc1_h1h1 | BOUNDED | h1 ok=531123, conn-flood ok=3,881,211, **err=0** | rss/fds/accept_inflight bounded |
| sc1b_h1h2 | BOUNDED | h1 ok=438092, conn-flood ok=3,915,796, **err=0** | rss/fds/accept_inflight bounded |
| sc2_h2h2 | BOUNDED | h2 ok=1,628,902; **rapid-reset ok=11,756,849 err=0** | CVE-2023-44487 defense holds; rss/fds bounded |
| sc3_slowloris | RSS-ramp* | h1_baseline ok=847191 err=0 | fds=209 flat, accept_inflight=148 flat (see *) |
| **sc4_modeb** (F-S20-1) | **BOUNDED** | **quic_load ok=4936 err=0** (S20: 0/23349) | conns≤4, streams≤4, rss/fds flat — **0 wedges** |
| sc4b_modeb_healthy | BOUNDED | quic_load ok=222436 err=0 | conns/streams/rss/fds bounded |
| **sc5_modea** (F-S20-2) | **flows/fds BOUNDED, evicted=6490** | quic_load ok=6617 err=0 (streams flow now) | flows bounded, **evicted_total rose 6490**; RSS see * |
| sc6_413teardown | BOUNDED | oversize_teardown ok=159735 **err=129918** (teardown raced head — by design), mid-stream ok=1,048,661 err=0 | rss/fds/accept_inflight bounded; **panic=0 → CF-S19 closed** |

**The two findings, PROVEN by the completed re-soak:**
- **F-S20-1 (sc4_modeb):** `ok=4936 err=0` over the run — the 4-concurrent-stream
  Mode B relay completes every session, **0 wedges**, state bounded. S20's
  `ok=0/err=23349` (100% stall) is gone. The defect was the load client, now fixed.
- **F-S20-2 (sc5_modea):** flows + fds BOUNDED with **evicted_total=6490** (the
  idle reaper actively reclaims — was **0** in the S20 leak), streams now flow
  (ok=6617 err=0). vs the S20 leak: flows→56457, fds→28240, RSS→331MB, evicted=0.

*\*RSS-ramp note (sc3, sc5):* both flag RSS "DRIFT" — the analyzer's
warmup-trimmed median-thirds + monotone heuristic trips on a one-time
**ramp-then-plateau** within the 240s window (the glibc allocator high-water
from an early connection-churn spike, then flat — NOT unbounded growth). Both
have all STATE metrics BOUNDED + panic=0. sc3's slowloris/H1 path is **byte-
identical to S20** (this session changed only passthrough Mode A, the QUIC load
client, and the sc6 teardown timing); S20's 90-min run showed sc3 RSS plateaus.
A 900-s confirmation run (isolated, non-saturated) registers the plateau as
BOUNDED for both — see §5b.

### 5b. RSS-plateau confirmation (900s, isolated) — COMPLETED (R15)
Re-ran the two RSS-ramp scenarios isolated for 900 s on a quiet box (load <7) so
the plateau dominates the analyzer's window. Archived:
`audit/soak/s21-soak-data/rss-plateau-900s/`.

- **sc5_modea (F-S20-2) — BOUNDED (900s, 61 samples), no asterisk.** `rss_kb`
  BOUNDED (last-third median 75796 vs first-third 76038 = **−0.3%**; RSS tail
  DEAD-FLAT at 75796 KB across t=825–885 s); flows BOUNDED (226→107), fds
  BOUNDED (124→62), **evicted_total=9895** (active reaping throughout), panic=0.
  vmhwm BOUNDED too (RSS plateaued early). **F-S20-2 is unimpeachably fixed:
  bounded flows/fds/RSS with continuous eviction — vs the S20 leak (330 MB
  monotone, evicted=0).**
- **sc3_slowloris — current RSS BOUNDED (+8.8%, flat tail ~23300 KB), fds=209
  flat, accept_inflight=148 flat, panic=0.** The only DRIFT-flagged metric is
  `vmhwm_kb` (the peak RSS high-water mark — monotone-by-CONSTRUCTION; it records
  max-ever RSS and can never decrease, so it is NOT a leak indicator). The
  current-RSS leak indicator is BOUNDED. sc3's slowloris/H1 path is byte-
  identical to S20 (this session touched only passthrough Mode A, the QUIC load
  client, and sc6 teardown timing); S20's 90-min run plateaued. This is the pre-
  existing slowloris allocator-arena slow-creep, NOT a regression (R3) and NOT a
  leak. (CF-S21-VMHWM-METRIC: the analyzer should treat `vmhwm_kb` as a peak
  gauge, not a monotone-drift candidate — minor carry-forward.)

**Re-soak verdict: CLEAN.** All 8 scenarios panic=0; all connection/flow/stream/
accept STATE BOUNDED; current-RSS BOUNDED everywhere; both fixed paths proven
(F-S20-1 sc4_modeb ok=4936 err=0 / 0 wedges; F-S20-2 sc5_modea bounded +
evicted=9895). S20's clean scenarios remain bounded (R3); CVE-2023-44487 defense
held under 11.76 M rapid-resets.

---

## 6. R1 ×3 regression gate (changed tree) — PENDING

_[fmt/clippy/×3 on the final committed tree — filled at Phase 4.]_

---

## 7. CF-S19-TLS-TEARDOWN-413 disposition

S20's sc6 sent oversize **HEADERS** (→ the 64 KiB header-limit 4xx path), NOT an
oversize **body** (→ the actual `413 PAYLOAD_TOO_LARGE` path at
`MAX_REQUEST_BODY_BYTES` = a hardcoded 64 MiB) — so S20 never exercised the 413
path (explaining "err=0, didn't reproduce"). **Mechanism:** the error head for
EVERY `error_response` (4xx/413/502) flows through the identical fully-buffered
`Bytes` body returned to hyper (`h2_proxy.rs::error_response`) — hyper owns the
H2/TLS flush; there is NO 413-specific gateway flush code that could race a
teardown. The only 413-specific code is the >64 MiB body buffering BEFORE the
flush, unrelated to teardown timing — and a load-scale 64 MiB-body flood would
saturate the box (the S20 anti-pattern). So the "gateway H2/TLS teardown races
the 413 head" suspicion does not correspond to gateway-side code.

**Sharper trigger (cheap, header path):** sc6's teardown timing now sweeps the
abort delay across 0/1/2/4/8 ms (0 ms drops the connection while the error head
is still in flight), aggressively racing the SHARED error-head flush window.
Re-soak verdict: §5 (sc6 bounded + panic=0 ⇒ CF-S19 closed with mechanism).

---

## 8. S22 handoff

With both soak findings resolved + a clean re-soak, the **9 cells + Mode A +
Mode B core is now stress-tested stable** — a defensible **shippable v1**.
Stress-tested core ≠ full spec. Remaining for FULL spec (S22+):

- **h3spec conformance pass** (expect 5–15 findings) — next session.
- **CF-S15-PASSTHROUGH-RETRY-ODCID** (NEW priority, surfaced by S21): with
  `mint_retry=true` (the production default), Mode A passthrough grants the
  client **0 application streams** end-to-end (the backend rejects the post-Retry
  ODCID transport param). A genuinely-LARGE Mode A architectural item — needs a
  "Retry Service"/PROXY-protocol-for-QUIC sidecar (token-embedded ODCID the
  backend extracts on verify). Its own future workstream (owner ruling: do NOT
  open in S21). Until then, Mode A carries streams ONLY with `mint_retry=false`
  (trusted-network/test escape), trading the §6.5 Initial-flood defence to the
  backend.
- WS-over-H2 (RFC 8441) + H3 (RFC 9220); gRPC-over-H3 conformance.
- Mode A deferred perf tiers (io_uring v1.1, XDP v1.2).
- Harness lesson banked: the lb-soak QUIC load client now honours `stream_send`
  partial writes; future soaks測 the gateway correctly (and a load/"control"
  must be proven NON-vacuous before it can argue a component is "unaffected").

Carry-forward (unchanged): CF-DEP-1 (Dependabot — owner), CF-IGN-1 (16 inherited
`#[ignore]` tests — characterize before h3spec), CF-FCAP-MARGIN, F-ESC-1
(multi-kernel CI lane), N-1 (jumbo-MTU).

## VERDICT
_[filled from the completed re-soak (§5/§5b) + Phase 4 gate (§6) + independent
verification (§9).]_

## 9. Independent verification (author ≠ verifier)
_[filled by the verifier on the COMPLETED data.]_
