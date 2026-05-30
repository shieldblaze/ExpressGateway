# S20 — Soak findings (working notes)

Status: WIP during the soak run. Mechanisms below are proven from CODE +
the smoke-validation repro (deterministic). The SOAK VERDICTS (does the Mode B
stall *leak state* over the full run? does Mode A growth *plateau at the cap or
keep climbing*?) are marked **PENDING-COMPLETED-RUN** — read ONLY from the
finished time-series (R15). Independent reproduction (R13) is post-soak.

The smoke surfaced these BEFORE the long soak; the long run characterizes their
stability impact over time.

---

## Finding F-S20-1 — Mode B multi-concurrent-stream relay STALL (priority #1, S18 lineage)

**Severity tier (R6):** STABILITY / LIVENESS (a stalled stream; leak-or-not
PENDING-COMPLETED-RUN).

**Symptom (reproduced, deterministic):** With ≥2 concurrent client bidi streams
on ONE Mode B connection, each sending `payload + immediate FIN`, the relay
reliably fails to deliver ONE stream's echo+FIN back to the client; the client
times out (`relay timeout / closed (streams left: N)`). Bisected in smoke:
- 1 conn, 1 stream, 0 dgram → **OK** (`quic_load ok=3352`)
- 1 conn, **4 streams**, 0 dgram → **FAIL** (`ok=0`, "streams left: 1")
- 1 conn, 1 stream, 8 dgram → OK (`ok=1270`)  ⇒ datagrams not the trigger
- 12 conns, 1 stream, 0 dgram → OK (`ok=840`) ⇒ concurrency not the trigger
⇒ **trigger = multiple concurrent bidi streams per connection.**

**Isolation (rules out apparatus):** the IDENTICAL client + 4 streams works
end-to-end through **Mode A** (`sc5 ok=3599`), so the QUIC client and the echo
backend are correct. The differentiator is the gateway's Mode B raw-stream
relay. Gateway logs confirm both legs establish ("re-originated upstream
connection established (two distinct conns)").

**Mechanism (from code — `crates/lb-quic/src/raw_proxy.rs`):** `relay_streams`
(L823) admits readable sids on both legs (well under the 256 cap), processes
ALL tracked sids via `pump_dir` both directions (L851-871), and reclaims only
streams whose BOTH halves `is_complete()` (L878). The S18 fix
(`!half.src_fin_seen` read gate, commit 1414d656) addressed FIN/tail loss on
the upstream→client (`u2c`) leg for a single stream. The smoke shows a
**deterministic 1-of-N stall under concurrent streams**, consistent with a
FIN/tail not propagating for one stream's `u2c` half — the same RELAY-STALL
class S16→S18 fought, now surfacing under multi-stream concurrency that the
deterministic s16_b2_multistream test did not exercise in this
all-streams-open-with-immediate-FIN pattern.

**Still to prove (Phase 3, post-soak):** WHICH stream stalls (likely the
highest sid) and whether the stalled stream's DATA fully arrived but the FIN
did not (FIN-loss → S18 lineage) vs partial data (tail-loss). Method: an
enhanced diagnostic client reporting per-sid received-bytes + fin-seen. Do NOT
rebuild the soak's binaries mid-run.

**Soak question (PENDING-COMPLETED-RUN):** sc4_modeb (4-stream) sustains this
stall under load for the full run — does the stalled-stream state LEAK
(quic_modeb_connections / streams_active / RSS climb) or stay BOUNDED? Smoke at
t≤90s showed connections steady at 12, streams_active=1, RSS flat — *looks*
bounded, but the verdict is the completed run's time-series.

**Disposition:** characterize + verify (R13) this session; FIX is S21 (it is a
relay-logic correctness fix in product code — builder-1 territory, needs a
deterministic multi-stream regression test; not a cheap one-liner to land
safely mid-soak-session without its own R13 burst proof).

---

## Finding F-S20-2 — Mode A passthrough flow/fd/task retention (no idle reclaim) (priority #4)

**Severity tier (R6):** STABILITY / RESOURCE-RETENTION (bounded at 2×cap, but
the bound is large and resources are held for dead connections;
leak-vs-plateau PENDING-COMPLETED-RUN).

**Symptom (reproduced):** Under sustained short-lived QUIC connection churn
(~180 conns/s), the passthrough flow table + the gateway's fd count + RSS grow
monotonically. Smoke t=20s: flows 3222→7186, fds 1628→3610, RSS 37→59 MB.
Soak liveness t=90s (NOT a verdict): flows≈16974, fds≈8504, RSS≈104 MB.

**Mechanism (from code — `crates/lb-quic/src/passthrough.rs`):**
- Each `FlowEntry` (L193) holds an `Arc<UdpSocket>` `backend_sock` (one fd per
  flow) and spawns TWO pump tasks (forward L804 + reverse L820).
- Reclamation is **LRU-eviction ONLY**, triggered when
  `table.len() >= max_quic_connections * 2` (= 200_000 for the default 100k)
  (L714-737, `evict_oldest` L537). There is **NO idle-timeout sweep** — the
  only `tokio::time` in the file is a per-recv 2 s timeout (L1418), not a
  table sweep.
- Passthrough CANNOT observe a client close (the QUIC CONNECTION_CLOSE is
  encrypted; the LB never decrypts — by design). So a flow for a CLOSED
  connection persists, holding its backend socket fd + 2 tasks, until the table
  fills to 2×cap and LRU evicts it.

⇒ Under churn the flow table climbs toward 200k, each entry pinning an fd + 2
tasks, regardless of how few connections are actually live. Bounded (at 2×cap)
but the bound is enormous and there is no idle reclamation; on a host with a
low fd ulimit this exhausts fds long before the flow cap.

**Deferral check:** no documented deferral for passthrough idle flow-reclaim
found in `audit/deferred.md` / `audit/quic/` (grep empty). Appears to be a
genuine gap, not a known carry-forward. (Confirm in Phase 3.)

**Soak question (PENDING-COMPLETED-RUN):** does sc5_modea plateau at ~2×cap
(LRU steady-state, evicted_total rising) — confirming the bound — or climb
unbounded (a true leak)? The completed time-series decides; the *expected*
shape from code is climb-to-200k-then-plateau.

**Fix (S21):** add a periodic idle-timeout sweep that evicts flows whose
`last_seen_ms` exceeds an idle threshold (≈ the QUIC idle timeout) — the
standard stateless-passthrough reclamation (Katran/Pingora-style). Bounds the
table by the LIVE connection count, not 2×cap. Not landed this session (product
change needing its own design + regression test).

---

## Clean-scenario expectations (evidence PENDING-COMPLETED-RUN)

- sc1_h1h1, sc1b_h1h2 (H1 + conn-flood): expect BOUNDED (smoke: RSS plateau,
  fds/accept_inflight oscillate bounded, 0 err).
- sc2_h2h2 (H2 + rapid-reset + stream-flood): expect BOUNDED; rapid-reset cap +
  max_concurrent_streams hold (smoke: ok=200k/1.07M, RSS flat).
- sc3_slowloris: expect BOUNDED (smoke: accept_inflight steady 388, fds 530
  flat — header/body timeouts reap; healthy baseline ok=14461).
- sc4b_modeb_healthy (1-stream Mode B): expect BOUNDED — the clean Mode B
  bounds-under-churn baseline (priority #2).
- sc6_413teardown: smoke err=0 at 18s (no teardown-race observed at that
  scale) — the full run + scale may surface CF-S19-TLS-TEARDOWN-413; PENDING.
