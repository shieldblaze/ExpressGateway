# SESSION 27 INC-5V — FINAL independent sign-off: WS-over-H2 (RFC 8441)

Independent verifier (did NOT author INC-3/4/5). Branch
`feature/websockets-s27`. All verdicts from COMPLETED scoped runs (R15).
Tip verified at `bc162fb8`.

## Per-task verdicts

### 1. Gating holds (the security disposition) — PASS
- Code: both gating points confirmed — SETTINGS advertise behind
  `if self.h2_extended_connect_enabled` (`h2_proxy.rs:841-843`); intercept
  behind the same flag as the FIRST `&&` term (`h2_proxy.rs:1055-1061`), so
  when off `is_h2_extended_connect` is never evaluated and
  `handle_ws_extended_connect` is never reached. Fall-through: a gate-off
  CONNECT+`:protocol` drops to the regular request path, which NEVER arms
  `hyper::upgrade::on` (the only place it is armed is inside the gated
  handler) → no tunnel, no relay. Default OFF end-to-end:
  `lb-config WebsocketConfig.h2_extended_connect=false`
  (`lb-config/src/lib.rs:725,738`) → `main.rs:1020` →
  `H2Proxy` field default `false` (`h2_proxy.rs:516,554`).
- Tests (re-run): `ws_h2_gated_off` x3 GREEN (`2 passed`). Non-vacuous: a real
  raw-h2 extended CONNECT yields no 2xx tunnel AND a counting backend proves
  `dialed==0`; the negotiated `is_extended_connect_protocol_enabled()` bit is
  read directly and asserted false. `lb-config` opt-in default test GREEN;
  `h2_connect_protocol_settings` gate code-presence GREEN.
- Adversarial: no path reaches `proxy_frames`/the dial with the flag off.

### 2. F-S27-1 still closed — PASS
- `ws_h2_upgrade_defer` x3 GREEN (`3 passed`) WITH the gate ON
  (`.with_h2_extended_connect(true)`, verified — so non-vacuous).
- Ordering spot-check after INC-3/4/5 churn: `handle_ws_extended_connect`
  (`h2_proxy.rs:1363+`) still dials + handshakes INLINE under
  `timeout(self.timeouts.header, ...)` and returns 502 (refused) / 504
  (dial-fail / elapsed) BEFORE the `200` is built (`h2_proxy.rs:1530-1538`).
  The new INC-4 `:scheme`/`:path` 400s sit BEFORE the dial. Independently
  reverted+re-confirmed load-bearing at INC-2V (`inc2v-independent-loadbearing.txt`).

### 3. LOW fixes correct — PASS
- `:scheme`/`:path`: `ws_h2_conformance` GREEN —
  `extended_connect_missing_scheme_is_400` and
  `extended_connect_missing_path_rejected_no_dial` (the latter asserts the
  backend is NEVER dialed → load-bearing). Was INC-2V PARTIAL, now PASS
  (`h2_proxy.rs:1399-1416`, removes the old silent `/` default).
- Trace-parity: `upstream_ws_handshake_carries_child_traceparent` GREEN;
  recorded upstream handshake header
  `00-0af7651916cd43dd8448eb211c80319c-a398fd5e41686646-01` — trace-id
  preserved (== client `0af7...319c`), parent-id `a398...6646` != client's
  `b7ad...3331` (LB span is the new parent). Non-vacuous (backend records the
  actual header). H1 parity (ROUND8-OPS-06).

### 4. R8 reconfirm (corrected scope) — see table below
- `ws_r8_backpressure_plateau`: H1 case GREEN (plateau 17–18 / 2048, x3);
  H2 case correctly `#[ignore]`d (CF-S27-2). `ws_h2_r8_backpressure`
  property (i) GREEN x3 (VmHWM volume-independent). `ws_h2_burst` R13 GREEN x3
  (fd 12→12, zero leak).
- I independently verified the corrected root cause: hyper `H2Upgraded` →
  `mpsc(1)` → `UpgradedSendStreamTask` → `h2::SendStream::send_data` buffers
  until window capacity (h2-0.4.13 share.rs:49-57), not bounded by
  `max_send_buf_size` for upgraded streams → relay `send().await` never parks
  on H2 → unbounded. H1 uses raw TCP whose `poll_write` surfaces `WouldBlock`
  → relay parks → bounded. My original INC-2V "H1 identical" was a false
  positive (corrected in `s27-r8-ws-proof.md` banner).

#### Corrected R8 status table

| Path | (i) bounded peak (volume-indep) | (ii) true backpressure (slow consumer) | Net |
|---|---|---|---|
| WS-over-H1 (shipped, default ON) | PASS | **PASS / BOUNDED** (plateau 17–18 / 2048 flood, x3) | SAFE, ships |
| WS-over-H2 (opt-in, default OFF) | PASS (VmHWM flat across 10x volume) | **FAIL — carried as CF-S27-2** (unbounded in hyper `H2Upgraded`/`h2::SendStream`; INC-3 added bounded write-buf + anti-hang Close-1008 as defense-in-depth) | gated OFF |

NO false "H2 backpressure PASS" claim is made. H2 (ii) is honestly FAIL,
carried as CF-S27-2, and mitigated by the off-by-default gate.

### 5. R3 no-regression — PASS
`h2_proxy_e2e` (5), `ws_proxy_e2e` (7), `round8_ws_upgrade_defer` (4) all GREEN.

### 6. Audit docs updated — done (this commit)
- `s27-rfc8441-conformance.md`: rewritten — §3 advertise only when opted-in
  (gated, CF-S27-2 caveat); §4 `:scheme`/`:path` PARTIAL→PASS; deferred-200
  PASS; trace-parity PASS row added; line numbers refreshed.
- `s27-r8-ws-proof.md`: prepended a CORRECTION banner + corrected Summary —
  the original H1 attribution / `WsConfig` root-cause are SUPERSEDED by the
  `1a308ac3` reconciliation (H1 SAFE; H2-only; root cause = hyper upgrade
  path). Audit trail is now internally consistent.

## FINAL WS-over-H2 VERDICT — READY TO SHIP (as gated/opt-in)

WS-over-H2 is **READY to ship as part of S27** under the honest
"implemented but gated OFF by default, opt-in for trusted clients" story:
- F-S27-1 (premature 200) FIXED + load-bearing-verified, intact post-churn.
- RFC 8441 §3/§4/§5 conformant; `:scheme`/`:path` enforced; trace-parity at
  H1 parity.
- F-S27-2 (no H2 tunnel backpressure) HONESTLY carried as CF-S27-2 and
  neutralized for the default deployment by the off-by-default gate
  (independently proven to hold: no advertise, no tunnel, no dial when off).
- WS-over-H1 ships ON and is bounded/backpressured (proven).

### Residual / NO NEW BLOCKER
- CF-S27-2 (H2 tunnel backpressure) remains OPEN but is owner-dispositioned
  (gated off) — NOT a ship blocker for S27. An operator that flips
  `h2_extended_connect = true` accepts the documented DoS exposure
  (mitigated, not eliminated, by INC-3's bounded write-buffer + the anti-hang
  Close-1008 guard, which bounds how LONG a wedged write hangs but not the
  in-flight `h2::SendStream` buffer). This trade-off should be called out in
  operator docs for the opt-in flag.
- No new finding from INC-5V.
