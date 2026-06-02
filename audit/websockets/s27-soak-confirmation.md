# S27 Phase 3 — Independent confirmation of the WebSocket relay soaks (verifier-p3)

**Verifier:** verifier-p3 (independent — did NOT author the soak code).
**Scope:** confirm the two completed WS soaks prove the relay leak-class (F-S20-2)
property is BOUNDED over H1 + H2, citing the committed artifacts only (R15).
**Method:** READ-ONLY (cat/grep/Read). NO cargo run of any kind (the lead's ×3
gate holds the shared `eg-target` lock). Every verdict below is sourced from a
file path + line/field, cross-checked against the raw CSV.

Artifacts:
- sc8_ws_h1  (WS-over-H1)            — commit `5041325d` — `audit/soak/s27-soak-data/sc8_ws_h1/`
- sc8b_ws_h2 (WS-over-H2, RFC 8441)  — commit `749a2306` — `audit/soak/s27-soak-data/sc8b_ws_h2/`

---

## 1. Completed, not killed — PASS (both)

| check | sc8_ws_h1 | sc8b_ws_h2 |
|---|---|---|
| `.soak_complete.marker` present | yes (`overall=BOUNDED`) | yes (`overall=BOUNDED`) |
| CSV data rows | 73 (74 lines incl. header) | 73 (74 lines incl. header) |
| run.log final line | `DONE scenario=sc8_ws_h1 samples=73 overall=BOUNDED` | `DONE scenario=sc8b_ws_h2 samples=73 overall=BOUNDED` |
| heartbeats t=0s → t=360s | full 0..360 @5s, contiguous | full 0..360 @5s, contiguous |

Both ran the full 360 s, emitted the analyzer block, wrote the marker, and ended
with `DONE … overall=BOUNDED`. Neither was truncated/killed.

## 2. BOUNDED + panic=0 — PASS (both), CSV-eyeball confirmed

I did NOT trust the summary; I read the raw CSV columns and confirmed the
analyzer's BOUNDED verdict matches the raw series. The analyzed columns are the
fixed `BASE_COLUMNS` (rss_kb, vmhwm_kb, fds, threads — `sampler.rs:19`) + the
per-scenario gauges (here only `panic_total`). Trend bands are the SHARED
defaults (`timeseries.rs:70` — band 10 %, monotone_min 0.60, warmup_frac 0.10).

**sc8_ws_h1** (`sc8_ws_h1.csv`, `.verdict.json`):
- `fds`: steady-state plateau ~39–43 the entire run (cold t=0 = 11 is
  warmup-trimmed). last-third median 41.5 vs first-third 41 (+1.2 %), monotone
  58 % → BOUNDED. **Does NOT ratchet** — it sawtooths 35↔43 around a flat level.
- `rss_kb`: last-third 15260 vs first-third 15300 (**−0.3 %**), slope
  −3.3/sample. Flat. `vmhwm_kb` +1.2 % (peak gauge, plateaus at 16708 by t≈195s).
- `threads`: flat 9 (after the t=0 bootstrap 10). `panic_total`: 0 start→end.
- Load: `ws_sustained ok=137075 err=0`, `ws_churn ok=5436016 err=0`.

**sc8b_ws_h2** (`sc8b_ws_h2.csv`, `.verdict.json`):
- `fds`: pinned at 43 nearly every sample (dips to 39–42 occasionally) →
  last-third median 43 == first-third 43 (**+0.0 %**), slope ≈ 0. **Flat, no
  conn leak.**
- `rss_kb`: +5.5 % (19964→21054 last-third median; max 21964), monotone 48 %
  (NON-monotone → not a leak signature). `vmhwm_kb` +5.2 %, plateaus at 22492 by
  t≈325s (a peak gauge; bounded, not climbing at end). The +5 % is a settling
  band on the heavier TLS+H2 path, not a ratchet — RSS is non-monotone and the
  last ~70 s oscillates around ~21 MB.
- `threads`: flat 9. `panic_total`: 0 start→end.
- Load: `ws_sustained ok=99331 err=0`, `ws_churn ok=192632 err=0`.

A real leak signature (high rel_growth AND high monotone on fds/rss) is absent in
both. grep for real error markers (`MISMATCH|ECHO|[ws_…|ERROR|panicked|
panic_total=[1-9]`) in both run.logs → **zero matches**.

## 3. Non-vacuity — PASS (both): drives the REAL relay, byte-verifies, churns clean

**(a) REAL gateway relay, nothing mocked.** The binary wires the relay from the
config block I read in the work dirs:
- sc8_ws_h1 `gateway.toml`: `protocol = "h1"` + `[listeners.websocket] enabled =
  true` + a real `[[listeners.backends]]`. `crates/lb/src/main.rs:940-942`:
  `build_h1_proxy` calls `with_websocket(WsProxy::new(...))` when the block is
  present → `WsProxy::proxy_frames` (`ws_proxy.rs:256`) runs end-to-end.
- sc8b_ws_h2 `gateway.toml`: `protocol = "h1s"` (TLS) + `h2_extended_connect =
  true`. `main.rs:1013-1020`: `build_h2_proxy` calls `with_websocket(...)` AND
  `with_h2_extended_connect(ws.h2_extended_connect)` → RFC 8441 extended CONNECT
  → 200 → the same `proxy_frames` relay onto the H1 backend.
- Backend is a genuine RFC 6455 echo origin (`backends.rs:179
  spawn_ws_h1_backend` → `tokio_tungstenite::accept_async`, echoes Text/Binary
  `msg` back verbatim, `:205-209`). A relay that corrupted bytes would be caught
  downstream.

  Path is therefore: load client → real gateway listener → `proxy_frames` →
  real WS echo backend → back. Nothing is stubbed.

**(b) BYTE-VERIFIES the echo (the integrity discriminant).** Payloads are
seeded, non-repeating (`(k + seed/i) % 251`, sizes from `BODY_SIZES = [0, 256,
4096, 65536]`), so a short-circuit/zero echo cannot pass. The compare is
explicit:
- H1 sustained `loadgen.rs:210` `WsEcho::Bytes(b) if b == payload => stats.ok()`.
- H1 churn `loadgen.rs:303` `Some(b) if b == payload => served += 1`.
- H2 sustained `loadgen.rs:633` `WsEcho::Bytes(b) if b == payload => stats.ok()`.
- H2 churn `loadgen.rs:575` `Some(b) if b == payload => served += 1`.
The 65536-byte payload exceeds a single H2 DATA / large WS frame, so the relay's
multi-frame chunking + reassembly is genuinely exercised (not a 1-byte echo).

**(c) Churn does real open→relay→CLEAN-close cycles (the fd-reclaim probe).**
`ws_h1_open_close_cycle` / `ws_h2_open_close_cycle` handshake, run N
byte-verified echoes, then `ws.close(None)` AND drain the closing handshake
(`loadgen.rs:307-315`, `:582-587`) — a CLEAN teardown, not an RST, which is
exactly what exercises the gateway's tunnel-reclaim path. With a leak, fds would
ratchet across cycles; it does not (§2).

**(d) NOT a vacuous all-close pass.** ok counts are large with err=0:
sc8_ws_h1 churn ok≈5.44M / sustained ok≈137K; sc8b churn ok≈193K / sustained
ok≈99K. Each ok is one byte-verified round-trip. A relay that closed immediately
on connect would yield ok≈0. The volume proves sustained real relaying.

## 4. The err=40 attribution (sc8b first run) — SOUND

The unified `WsEcho` enum (`loadgen.rs:157`) splits two outcomes:
- `WsEcho::Bytes(b)` with `b != payload` → **`stats.err()` + `bail!("ECHO
  MISMATCH (relay integrity defect)")`** (`:212-215` H1, `:635-639` H2). A
  genuine byte mismatch STILL counts as err and aborts the session — the fix
  did NOT mask relay errors.
- `WsEcho::Closed` (clean `Message::Close` / `None` / read-timeout mid-loop) →
  the sustained worker `return Ok(())` and the outer loop re-establishes; NOT
  counted as err (`:221`, `:646-649`). This is a connection-LIFECYCLE event
  (gateway-drained tunnel / idle close / end-of-run cancel landing between send
  and read), not an echo-integrity failure.

Independent read verdict: **SOUND.** The discriminant that flags a defect (the
byte-compare) is untouched and load-bearing; only the benign clean-close path
was reclassified from err→reconnect. The commit message for `749a2306`
corroborates the first-run 40: a `WS_DEBUG` run showed every one was a clean
close with ZERO "ECHO MISMATCH". The large err=0 ok-count (§3d) guards against a
vacuous all-close pass — soak-eng distinguished lifecycle-close from
relay-defect; it did not paper over an error.

*Residual (minor, noted not blocking):* `Some(Err(_)) => WsEcho::Closed`
(`:204`, `:626`) maps a transport-layer read error to a benign reconnect, so a
hypothetical relay defect that surfaced as a transport Err (rather than wrong
bytes or a clean Close) would not increment `err`. This does NOT weaken the
leak-class target of the soak, and any persistent such defect would crater the
ok-rate / show as an fd/RSS anomaly (neither observed). Acceptable for a
leak-class soak; the byte-compare remains the integrity discriminant.

## 5. fds discriminant + accept_inflight cross-check — SOUND

- **accept_inflight is NOT in the CSV at all** for these scenarios.
  `ws_gauges()` (`eg-soak.rs:865`) returns `["panic_total"]` only; the CSV
  header is `t_secs,rss_kb,vmhwm_kb,fds,threads,panic_total` (no
  accept_inflight). So there is nothing to "ratchet vs sawtooth" in the data —
  it was simply not scraped. (One doc comment at `eg-soak.rs:681-683` says it is
  "scraped for visibility"; that prose is slightly stale vs the actual
  `ws_gauges()`, which omits it. Cosmetic — it does not affect the analyzed set
  or any verdict.)
- The stated rationale (`eg-soak.rs:848-864`) is sound: under open/close churn
  `accept_inflight` is a low-baseline sawtooth (live tunnels open+close between
  samples, dipping toward 0), which the relative-growth analyzer can mis-flag
  (a 0→2 wiggle reads as "+200 %" when the first-third median lands on a 0
  trough). `fds` has no low-baseline pathology and IS a `BASE_COLUMN`, always
  Trend-analyzed — every live tunnel pins a client fd + a backend fd, so a
  relay-task/connection leak ratchets fds up. A bounded fds series IS the
  no-leak proof. **fds is a valid leak discriminant.**
- **Analyzer NOT weakened.** `git show 5041325d -- timeseries.rs` and
  `git show 749a2306 -- timeseries.rs` are BOTH EMPTY — the analyzer math
  (`analyze` / `analyze_column`, band 10 %, monotone 0.60) was untouched by the
  soak commits. soak-eng only chose WHICH columns to analyze (`ws_gauges()` in
  eg-soak.rs), not the math. Confirmed.

---

## Overall verdict

**The WS relay leak-class (F-S20-2) is legitimately PROVEN bounded over H1 + H2.**

| dimension | sc8_ws_h1 | sc8b_ws_h2 |
|---|---|---|
| Completed (not killed) | PASS | PASS |
| BOUNDED (CSV-eyeballed) | PASS | PASS |
| panic = 0 | PASS | PASS |
| non-vacuous (real relay + byte-verify + clean-churn + large ok) | PASS | PASS |

- err=40 attribution: **SOUND** (lifecycle-close ≠ relay-defect; byte-mismatch
  still counts as err + bails; large ok-count guards vacuity).
- fds discriminant: **SOUND** (BASE_COLUMN, flat/no-ratchet on both;
  accept_inflight legitimately excluded — analyzer math unchanged).

**Residual doubt (low):** (1) the `Some(Err(_)) → Closed` transport-error
mapping is slightly under-sensitive (§4 residual) but does not affect the
leak-class proof. (2) one stale doc line about accept_inflight being scraped
(§5) — cosmetic. Neither blocks the BOUNDED verdict.

Recommendation: ACCEPT the two WS soaks as the leak-class evidence for the WS
relay over H1 and H2. (WS-over-H3 was not part of this soak ask.)
