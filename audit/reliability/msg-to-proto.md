# `rel` → `proto` — round 1 handoff

Four coordination items, all SIGTERM- or backpressure-shaped:

1. **GOAWAY / CONNECTION_CLOSE on SIGTERM.** My read: no path emits
   H2 GOAWAY or H3 CONNECTION_CLOSE during drain. Listener tasks are
   `JoinHandle::abort()`ed (`crates/lb/src/main.rs:1037`) and the per-
   connection task is torn down with the runtime. Please confirm or
   point me at the path I missed. RFC 7540 §6.8 / RFC 9000 §10.3
   compliance flag if confirmed.

2. **H2 security detectors** — `METRICS.md:103-107` rows say
   "detector exists but not invoked." Do
   `SettingsFloodDetector`, `RapidResetDetector`,
   `ContinuationFloodDetector`, `HpackBombDetector` actually fire
   inside the live H2 connection driver
   (`crates/lb-l7/src/h2_proxy.rs:400`)? If yes, what's missing for the
   metric emission? If no, we need a joint finding to wire them.

3. **QUIC actor-channel full policy.** `crates/lb-quic/src/router.rs:343`
   uses `mpsc::channel(ACTOR_CHANNEL_DEPTH=32)`. Is the producer side
   using `try_send` (drop on full) or `send().await` (block on full)?
   Drop-on-full is correct for UDP semantics; block would cause the
   datagram task to back-pressure all flows. Verify and document.

4. **`http_requests_total` labelling.** The counter is bumped once per
   *connection*, not per request (`main.rs:1186-1192`). For H2/H3
   that's a ~100× undercount. Proposed fix: pass a stats hook from
   `lb/src/main.rs` down to `lb-l7` proxy so per-request emission
   happens at the L7 layer. METRICS.md acknowledges this but no owner
   exists yet — suggest joint finding.

Pointers in `audit/reliability/round-1-inventory.md`: §1 F-18,
§4.4, §5.1, §6.1, §9.
