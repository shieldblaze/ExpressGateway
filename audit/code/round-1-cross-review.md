# Round 1 — Cross-Review Notes (`code` reviewer)

This file records disagreements, hand-offs, and explicit cross-checks
between `code` and the four other auditors (`sec`, `ebpf`, `rel`,
`proto`). It is **not** the findings register — those open in Round 2
under `audit/code/findings.md`.

## Messages dispatched (Round-1 close)

The `SendMessage` deferred tool is not present in this session
(verified via ToolSearch — no matches for `SendMessage`/`UpdateTask`).
In lieu of a real inter-agent channel, each teammate's hand-off
below is also mirrored in §6 of `round-1-inventory.md`. The
team-lead is asked to route these to the named teammate or confirm
acceptance.

### To `sec` (security-reviewer)

- **Q-CODE-1-01** — `lb-security` exports `SlowPostDetector`,
  `SlowlorisDetector`, `SmuggleDetector`, retry / 0-RTT, but
  `lb-l7/Cargo.toml` does **not** depend on `lb-security`. The H1/H2
  proxies do not appear to wire those detectors into the request
  path. Either point me at the wiring or accept this is an open
  gap.
- **Q-CODE-1-02** — Every atomic in the workspace is `Relaxed`. If
  any *enforcement* threshold on the security side (rapid-reset
  counts, per-IP rate limits, replay guard) depends on a counter,
  Relaxed is too weak. Need your sign-off per detector.
- **Q-CODE-1-08** — Fuzz corpora are 5-7 inputs per target. Are you
  planning HPACK/QPACK/chunked-transfer/varint/gRPC-frame/WS-frame
  fuzz targets in Round 2?
- **Q-CODE-1-09** — `lb-quic/router.rs:374` per-CID actor entries
  leak into `DashMap` if actor panics before `remove`. Reaped on
  next failed `try_send`. Acceptable to you under sustained Initial
  flood?

### To `ebpf` (kernel-ebpf-specialist)

- **Q-CODE-1-03** — `crates/lb-l4-xdp/ebpf/` is out-of-workspace
  with its own Cargo.lock and toolchain. My workspace clippy /
  fmt / MSRV sweeps do **not** cover it. Confirm you are linting
  it independently; if not, propose how to fold it into CI.
- **51 unsafe blocks** in `ebpf/src/main.rs` — every packet
  dereference uses `core::ptr::read_unaligned(addr_of!(...))` after
  a `ptr_at::<T>` bounds check. The bounds-check helper is the
  trust anchor; if you sign off on `ptr_at` / `ptr_at_mut`
  (`main.rs:234,246`), the rest follows by induction.
- **4 unsafe-impl-Pod** in `crates/lb-l4-xdp/src/loader.rs` —
  `FlowKey`, `BackendEntry`, `FlowKeyV6`, `BackendEntryV6`. Each
  is `#[repr(C)] Copy`. `FlowKey` carries `pad: [u8; 3]` to round
  to 16 bytes. Confirm the v6 variants are likewise padding-free
  on every target the kernel uses (no implicit alignment holes).

### To `rel` (reliability-engineer)

- **Q-CODE-1-04** — Cancellation discipline is *partial*. The TCP /
  H1 / H1s listeners spawned at `crates/lb/src/main.rs:701`
  receive no shutdown signal; only the admin HTTP listener
  (`lb-observability::admin_http`) and the QUIC listener / router
  use `CancellationToken`. SIGTERM today = drop runtime = abort
  all connections. Is graceful drain on your finding list?
- **Q-CODE-1-05** — The 1-second metrics sampler at
  `crates/lb/src/main.rs:892` loops forever with no cancel arm.
  Same question.
- The `_rotator` `Arc<Mutex<TicketRotator>>` is held inside the
  listener-mode enum and its background ticker exits when
  `strong_count <= 1`. This is a clean shutdown story for that
  task but does **not** generalize to the rest of the spawn fleet.

### To `proto` (protocol-expert)

- **Q-CODE-1-06** — `lb-h1` has zero workspace consumers; only the
  fuzz target uses it. Dead code, or staged for cross-verification
  against hyper? If alive, please add the dependency edge from
  `lb-l7` (or wherever you intend it).
- **Q-CODE-1-07** — `lb-balancer::Backend` and `lb-core::Backend`
  both carry `active_connections: u64` (atomic in `lb-core`,
  plain in `lb-balancer`). The balancer algorithms operate on the
  *snapshot* `u64` — never on the live atomic. This is a designed
  decoupling, but the rebalance interval is not documented. Is
  there a periodic sync I missed?
- HPACK / QPACK / chunked / varint / gRPC-frame: please confirm
  whether you want me to write the fuzz-target shells in
  Round 4 or if you'll own them.

## Disagreements

None yet — this is the discovery round. Disagreements will be
captured here in Round 2+ once findings exist.

## Acknowledgements

Team-lead: please confirm that absent a real inter-agent channel,
mirroring the hand-offs in the inventory's §6 is acceptable, or
route this file to each teammate manually.
