# SESSION 21 HANDOFF — Fix the S20 soak findings → clean re-soak

S20 built the chaos/soak suite (`crates/lb-soak`, permanent infra) and ran the
first full-system soak. The system upholds its R8 bounded-state invariants under
sustained hostile load (all clean scenarios BOUNDED, panic=0, CVE-2023-44487
defense held under 227M resets). The soak found **2 real defects**. S21 = FIX
them (each with a regression test + R13 where timing-sensitive), then run a
clean re-soak (use `scripts/soak/s20-run.sh` with the reduced concurrencies; do
NOT co-locate sc5 Mode A with the others — it is the resource polluter).

## Prioritized fix list

### 1. [HIGH] F-S20-1 — Mode B multi-concurrent-stream relay STALL
- **What:** EXACTLY 4 concurrent client bidi streams on one Mode B connection →
  the 4th/highest stream (sid12) gets ~1212/4096 echo bytes then wedges; 3
  streams is fine. Gateway-relay-specific (Mode A unaffected). Liveness bug —
  does NOT leak state (90-min soak: RSS/fd/conn/stream flat).
- **Why HIGH:** Mode B is the newest, headline-feature datapath; a real client
  opening ≥4 concurrent streams on one QUIC connection (normal H3) would see
  one stream silently stall. The s16_b2_multistream test passed because it did
  not exercise the 4-concurrent-streams-with-immediate-FIN pattern.
- **Repro (deterministic):**
  `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target QUIC_CONCURRENCY=1 QUIC_STREAMS=4 QUIC_DGRAMS=0 QUIC_DEBUG=1 eg-soak --scenario sc4_modeb --label r --duration-secs 25 --out /tmp/x`
  → `quic_load ok=0`, err detail `sid12=1212/4096`. STREAMS=3 → ok>0.
- **Investigation start:** `crates/lb-quic/src/raw_proxy.rs`
  `relay_streams` (L823) / `pump_dir` (L1275) / `run_dual_pump` (L459) under 4
  concurrent bidi streams. NOT a config-credit limit (front grants 16 streams /
  10 MB, upstream 64 / 10 MB). The 1212-byte partial + highest-stream-only +
  4-stream-knee are the signature — look for a per-pass / per-stream
  scheduling or readable-set interaction that starves the 4th stream's u2c
  (upstream→client) drain. The lead's diagnostic client (loadgen.rs, reports
  per-sid got/want) is the tool.
- **Fix shape:** relay-logic correctness fix + a DETERMINISTIC 4-concurrent-
  stream regression test (assert all 4 streams echo byte-identical incl. FIN).
  Verify with author≠verifier (R13).

### 2. [MEDIUM-HIGH / security-adjacent] F-S20-2 — Mode A passthrough flow/fd retention, no idle reclaim
- **What:** passthrough flows reclaimed ONLY by LRU at 2×`max_quic_connections`
  (200k default); NO idle sweep. Each flow pins a backend UDP socket fd + 2
  pump tasks. Under churn flows/fds/RSS grow monotonically (isolated run:
  0→56k flows, 11→28k fds, 8→330 MB RSS, evicted=0) and SATURATE the gateway's
  CPU on unreclaimed-flow recv-timeout wakeups BELOW the cap → effective
  connection-acceptance DoS at ~56k retained flows.
- **Why MEDIUM-HIGH:** a sustained stream of short-lived QUIC connections (or a
  spoofed-source flood that survives Retry) drives the table up; the gateway
  stops accepting new connections long before the configured cap, and on a host
  with a low fd ulimit it fd-exhausts. Bounded only by 2×cap (huge) + the
  saturation knee.
- **Repro:**
  `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target QUIC_CONCURRENCY=1 eg-soak --scenario sc5_modea --label r --duration-secs 90 --out /tmp/x`
  → flows rise monotonically, evicted_total stays 0.
- **Fix shape:** a periodic idle-timeout sweep task (e.g. every ~1 s) that
  evicts flows whose `last_seen_ms` exceeds an idle threshold (≈ the QUIC max
  idle timeout), in `crates/lb-quic/src/passthrough.rs` (the
  `last_seen_ms` AtomicU64 + `evict_oldest` machinery already exist; add the
  sweep that walks the table by age). Bounds the table by LIVE connections.
  This is the standard stateless-passthrough reclamation (Katran/Pingora). +
  regression test: flows reclaim to ~baseline after an idle window with no new
  packets.

### 3. [INVESTIGATE] CF-S19-TLS-TEARDOWN-413 — did NOT reproduce in S20
- The S19-escalated TLS-teardown-vs-413-head race did not surface under the S20
  oversize-413 + teardown injector (sc6: 2.4M attempts, err=0 over 90 min).
  NOT cleared. S21: build a more targeted trigger (precise mid-413-flush abort
  / concurrent TLS-layer teardown contention) and either reproduce + fix or
  prove it is no longer reachable on the current tree.

## After the fixes
Run a clean re-soak (reduced concurrencies; sc5 isolated). PASS = all scenarios
BOUNDED incl. sc4_modeb 4-stream (ok>0, no stall) and sc5_modea (flows reclaim,
fds bounded). Then the program's spec-conformance work (h3spec; WS-over-H2 RFC
8441 + H3 RFC 9220; gRPC-over-H3). ~2 sessions to stress-tested shippable v1
after S21's fixes + a clean re-soak.

## Carry-forward (unchanged from S20 brief)
CF-DEP-1 (Dependabot — owner), CF-IGN-1 (16 inherited #[ignore] tests),
CF-FCAP-MARGIN, F-ESC-1 (multi-kernel CI lane — pair with Mode A XDP tier),
N-1 (jumbo-MTU), Mode A deferred perf tiers (io_uring v1.1, XDP v1.2).
