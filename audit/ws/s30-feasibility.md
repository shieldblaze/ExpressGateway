# SESSION 30 — WS-over-H2 backpressure feasibility analysis (the pivotal artifact)

**Question (Phase 1 gate):** Can the WS-over-H2 tunnel path obtain
WINDOW-AWARE backpressure — so a STALLED consumer bounds gateway memory —
via a CONTAINED change that does NOT alter how hyper serves GENERAL
(non-CONNECT) H2 requests? Sanctioned direction (R7): "window-gated writes
on the raw `h2::SendStream`, bypassing hyper's `Upgraded` for the WS-tunnel
path ONLY."

**VERDICT: NOT-VIABLE this session** (for a contained, in-repo,
no-dependency-fork, no-broad-rearchitect fix). The sanctioned "raw
`h2::SendStream` for the tunnel path only" seam **does not exist** in
hyper's public surface. Getting that handle requires owning the entire H2
connection codec, which is precisely "changing general H2 serving" — the R3
crown-jewel risk and the R7(a) escalation trigger. Confirmed independently
by two readers of primary source (lead + independent verifier — see
`s30-feasibility-verifier.md`), converging on identical file:line evidence.

This is the honest Exit-Condition-(b) landing: WS-H2 stays GATED, the H2
stack is untouched (zero source change → zero R3 risk), CF-S27-2 carries
with a sharpened mechanism + the architectural fork escalated to the owner.

---

## Phase 0 baseline (all green; completed runs)

| Check | Result |
|-------|--------|
| Base tip | `main @ f5a04b2b` (S29, spec-complete) — confirmed |
| Branch | `feature/ws-h2-backpressure-s30` created + pushed |
| Disk / strays | 45 GB free (≥25), no S29 strays |
| `cargo build --workspace --all-features` | clean (exit 0) |
| **h2spec strict (`h2spec -S`)** | **PASS — exit 0 (146 passed / 1 applicability-skip / 0 failed). CROWN-JEWEL INTACT.** |
| **F-S27-2 reproduced** | see below |

### F-S27-2 negative-control reference (the load-bearing control, R13(c))

Completed run, `cargo test --test ws_r8_backpressure_plateau --release
-- --include-ignored --test-threads=1` (flood = 2048 × 256 KiB = 512 MiB at
a non-reading client; plateau ceiling = 256 msgs / ~64 MiB):

| transport | backend pushed / 2048 | in-flight | verdict |
|-----------|------------------------|-----------|---------|
| **H1 (shipped)** | **20 / 2048** | ~5 MiB | BOUNDED — PASS (backpressure works) |
| **H2 (gated)** | **2048 / 2048** | ~512 MiB | **UNBOUNDED — FAIL (the F-S27-2 DoS)** |

The H2 path absorbed the **entire 512 MiB flood** into gateway RAM at a
stalled consumer. This is the reference the negative control needs: any real
fix must flip H2 from 2048/2048 to a flat plateau < 256, volume-independent.
The H1 path proves the relay logic itself is correct — the gap is purely the
H2 transport's missing window gating.

---

## Root cause (re-derived from vendored source; both readers agree)

The relay `ws_proxy::proxy_frames` forwards via `client_tx.send(msg).await`
(ws_proxy.rs:392). Over H2 the client sink is tungstenite over hyper's
`Upgraded`, whose concrete inner IO is `H2Upgraded`
(hyper-1.9.0 `src/proto/h2/upgrade.rs:39`, declared `pub(super)`).

1. **`H2Upgraded::poll_write` gates only on a `mpsc::channel(1)`, never on the
   H2 window** (upgrade.rs:205 `self.send_stream.tx.poll_ready`; the channel
   is `mpsc::channel(1)` at upgrade.rs:21). On Ready it `start_send`s a
   `Cursor<Box<[u8]>>` and returns `Poll::Ready(Ok(n))` (upgrade.rs:222) —
   no flow-control consultation.

2. **`UpgradedSendStreamTask::tick` drains that mpsc into `send_data`
   UNCONDITIONALLY.** When the window is exhausted, `poll_capacity` returns
   `Poll::Pending`, which does `break 'capacity` (upgrade.rs:98) — the
   "no window" signal is **swallowed, not propagated**. Control falls through
   to `rx.poll_next` (upgrade.rs:116) → `me.h2_tx.send_data(...)`
   (upgrade.rs:118-120) regardless of capacity. `tick` returns `Pending` ONLY
   when the mpsc is empty (upgrade.rs:128), never because the window closed.
   ⇒ the mpsc(1) drains as fast as the task is polled ⇒ `poll_ready` is
   ~always Ready ⇒ the relay's `send().await` **never parks over H2**.

3. **`h2::SendStream::send_data` buffers UNBOUNDED with no window.** h2-0.4.13
   `src/share.rs:56-59` (doc): "There is no bound on the amount of data that
   the library will buffer"; `src/share.rs:326-331`: "this buffering is
   unbounded." Impl `src/proto/streams/prioritize.rs:218` pushes onto an
   uncapped `pending_send` `VecDeque`; each entry is a heap-owned buffer.

4. **hyper's `max_send_buffer_size` (we set 64 KiB) does NOT bound this.** It
   only computes the *reported* `capacity()` (h2 `stream.rs`:
   `available.min(max_buffer_size).saturating_sub(buffered)`); it never
   rejects/blocks `send_data`, and the upgrade `tick` bypasses capacity
   gating anyway (pt 2).

5. **Not fixed in newer hyper.** `diff hyper-1.9.0 hyper-1.10.1
   src/proto/h2/upgrade.rs` = a single cosmetic `.unwrap()`→`.expect()`;
   `tick` is byte-identical. A lockfile bump does NOT fix it.

---

## Seam analysis — why no contained fix exists

**(A) hyper builder/feature knob.** None. `max_send_buf_size` is already set
and structurally ineffective (pt 4). VERDICT: doesn't exist.

**(B) Recover the raw `h2::SendStream` via `Upgraded::downcast`.** NOT
feasible. `Upgraded::downcast::<T>()` requires naming `T = H2Upgraded`, which
is `pub(super)` (upgrade.rs:39) — un-nameable from outside hyper. And even if
it were nameable, `H2Upgraded` holds only the `mpsc::Sender` bridge
(upgrade.rs:26,46-49), **not** the `SendStream` — hyper moved the `SendStream`
into the private spawned `UpgradedSendStreamTask` (server.rs:495-503). There
is no accessor for the inner window-bearing stream.

**(C) Treat extended CONNECT as a normal flow-controlled body exchange**
(hyper's regular request/response body path DOES backpressure correctly —
that's why the H2→H1/H2/H3 cells pass R8 + h2spec). NOT possible: hyper
**hard-routes any CONNECT through the upgrade fork** before the service sees
it. server.rs:282-298 captures the request body into private
`ConnectParts.recv_stream` and hands the service `IncomingBody::empty()`;
server.rs:478-504 rejects a response body for a 2xx CONNECT
("CONNECT request with body not supported", RST) and forces `upgrade::pair`.
There is no flow-controlled-body alternative for a CONNECT stream.

**(D) Relay-level bounded in-flight wrapper** ("don't read next producer
frame until prior is drained"). Does NOT bound H2: `send().await` over
`H2Upgraded` returns as soon as bytes hit h2's unbounded buffer, not after
they leave the gateway. `proxy_frames` is ALREADY exactly this single-in-
flight design, yet H2 absorbed 2048/2048. VERDICT: doesn't bound H2.

**(E) Drive the `h2` crate's server directly for the WS path.** This is the
ONLY way to obtain the window-bearing `SendStream`. But the `SendStream`
exists only if you own the whole `h2::server::Connection`. **H2 multiplexes
regular requests AND extended CONNECT over a single connection**, and the
client decides per-stream — you cannot know in advance which connections
will carry a tunnel, and you cannot peel one stream out of a hyper-owned
connection. So you must route ALL H2 connections on a WS-enabled listener
through a hand-built `h2`-direct server that re-implements general request
serving (the 9 H2 cells, all h2spec conformance, the security thresholds,
PING/CONTINUATION/RST-flood defenses). That **is** "changing general H2
serving" = the R3 crown-jewel risk = the R7(a) "too invasive" STOP signal.

---

## The escalated architectural fork (R7 — OWNER decision)

There is no in-repo, contained fix. The real options, for the owner:

### Option 1 — STAY GATED (recommended)
Keep WS-over-H2 off-by-default; carry CF-S27-2 with this sharpened mechanism.
- **Risk:** none. Zero source change, h2spec 146/147 untouched, spec stays
  complete.
- **Coverage cost:** low. WS-over-H1 (RFC 6455) and WS-over-H3 (RFC 9220)
  both ship with TRUE backpressure (S27/S28), and gRPC-over-H3 too (S29). The
  ungated transports cover the real WS/streaming surface; RFC 8441
  WS-over-**H2** is the rare one (most WS clients use H1 Upgrade).
- An always-on time-bound mitigation is already present even when ungated:
  the relay's `read_frame` write-timeout (default 30 s) → Close 1008 reclaims
  a wedged tunnel — it bounds dwell by TIME, not a true window plateau, so it
  is a mitigation, not the fix.

### Option 2 — Vendored hyper fork via `[patch.crates-io]`
Patch `UpgradedSendStreamTask::tick` to gate `send_data` on real capacity
(propagate the `poll_capacity == Pending` as the task's `Pending` instead of
`break 'capacity` + unconditional `send_data`). ~10-20 lines, surgically
isolated to the upgrade path (general serving uses `PipeToSendStream`, which
already reserves capacity correctly — unaffected).
- **Risk:** a maintained third-party fork = supply-chain + maintenance burden
  + a dependency-governance decision (R7(b)). Best paired with upstreaming
  the fix to hyper (this is arguably a hyper bug). Until upstreamed, every
  hyper bump must re-apply the patch.
- After landing it, the R8 stalled-consumer plateau test would flip H2 from
  2048/2048 to a flat < 256, and WS-H2 could be un-gated.

### Option 3 — Broad rearchitect (direct `h2` server for WS listeners)
Re-host general H2 serving on the raw `h2` crate (Option E above).
- **Risk:** LARGE; directly endangers the h2spec 146/147 crown-jewel (R3).
  Not recommended.

---

## Recommendation
Per R6/R11(b): **Option 1 (stay gated).** The clean fix is genuinely not
feasible in-repo this session; a gated-but-honest WS-H2 beats risking the
mature, h2spec-passing H2 stack or shipping a fake/time-only backpressure
claim. Option 2 (upstream-then-patch hyper) is the path if WS-over-H2 ever
becomes a required transport. Decision is the owner's (R7).

## Provenance (R15)
All numbers from completed runs this session: F-S27-2 repro
(`ws_r8_backpressure_plateau`, H1 20/2048 PASS, H2 2048/2048 FAIL, 10.10s);
h2spec strict exit 0. Source citations are vendored under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Independent
verifier's parallel derivation: `s30-feasibility-verifier.md`.
