# Round 8 Phase C — Reconciliation (verify)

Tags 39 Round-8 findings against the 70 prior Round-2 findings (across
sec/code/ebpf/rel/proto) plus the Round-5/6/7 verification trails.

Legend:
- **NEW**     — prior audit did not examine this area.
- **MISSED**  — prior audit examined an adjacent surface but stopped
                short of the specific bug Round-8 found.
- **DISPUTE** — prior audit examined and dismissed; Round-8 disagrees.

---

## L7 (15 findings)

### ROUND8-L7-01 — Premature 101 on WS upgrade (critical, post-arb)
- **Tag: NEW**
- Prior coverage: nothing. PROTO-2 register covers H1/H2/H3 framing,
  hop-by-hop, smuggle matrix, 100-Continue (`PROTO-2-03`), but nobody
  examined `h1_proxy::handle_ws_upgrade`. CODE-2-01 wired the smuggle
  detector but did not look at the WS upgrade response ordering.
- The premature-101 + post-upgrade byte ownership bug class (Pingora
  GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh) is not referenced
  in any Round-2 file.

### ROUND8-L7-02 — Chunk-size hex parser accepts `+`, leading ws, unbounded digits
- **Tag: NEW**
- Prior coverage: `lb-h1::chunked` is consumed by the H1 parser. The
  Round-2 inventory lists `lb-h1` (CODE-2-15) as "no in-workspace
  consumer outside the fuzz target", and the fuzz target is named
  `crates/lb-h1/fuzz`. No prior finding interrogates the
  `try_read_size` predicate against the hyper GHSA-5h46-h7hh-c6x9 /
  HAProxy `h1_append_chunk_size` class.

### ROUND8-L7-03 — Empty header name silently accepted
- **Tag: NEW**
- Prior coverage: SEC-2-01 (`SmuggleDetector`) + PROTO-2-10 cover
  CL/TE duplicates / hop-by-hop / smuggling but reject by header
  **name** matching. Empty-name primitive was not in the smuggle
  matrix; `parse_headers_with_limit` byte-level validation never
  audited.

### ROUND8-L7-04 — XFF append clobbers all but first value
- **Tag: NEW**
- Prior coverage: Round-2 register has no XFF-related finding.
  `audit/protocol/SMUGGLE-MATRIX.md` did not look at header
  collapse / list-rule iteration. Confirmed by `grep XFF` /
  `grep x-forwarded-for` across all five Round-2 files: zero
  matches.

### ROUND8-L7-05 — No `headers_with_underscores` rejection
- **Tag: NEW**
- Prior coverage: none. The `_` ↔ `-` normalisation primitive
  (Envoy edge default `REJECT_REQUEST`, nginx `underscores_in_headers`)
  is absent from every Round-2 file.

### ROUND8-L7-06 — No `keepalive_requests` count cap
- **Tag: NEW**
- Prior coverage: CODE-2-05 + REL-2-09 cap *concurrent* inflight but
  not *lifetime requests per connection*. The nginx-100 / Pingora-0.8
  industry-standard count cap was not previously audited.

### ROUND8-L7-07 — No frame-level H2 read timeout
- **Tag: NEW**
- Prior coverage: SEC-2-03 covers H1 slowloris (`SlowlorisDetector`).
  H2 slowloris via per-frame timer (nginx `http2_recv_timeout`) was
  not audited. `h2_security.rs` defends rapid-reset, hpack-bomb,
  continuation-flood, settings/ping flood — six independent counters
  but no frame-arrival timer.

### ROUND8-L7-08 — Upstream H2 read timeout drops future without RST_STREAM(CANCEL)
- **Tag: NEW**
- Prior coverage: `Http2Pool` was touched by CODE-2-09 (acquire_async
  migration) but the RST_STREAM(CANCEL) semantics on app-level read
  timeout — Pingora 0.8.0 lesson 10 — were never in scope.

### ROUND8-L7-09 — No authority comma / control-char rejection in any parser
- **Tag: MISSED** (PROTO-2-15 / PROTO-2-18)
- Prior coverage: PROTO-2-15 (`SNI ↔ Host`) wired `check_sni_authority`
  at commit `444668d` (Round 6). That validator performs *agreement*
  check (case-insensitive, port latitude, IPv6-aware) but not *value
  sanitisation* — comma / whitespace / control-char rejection per
  HAProxy `BUG/MAJOR: http: forbid comma in authority` is missing.
  Prior coverage stopped at the SNI/Host comparator and never walked
  the predicate the comparator should use.

### ROUND8-L7-10 — Body over-read does not mark connection non-reusable
- **Tag: MISSED** (CODE-2-09)
- Prior coverage: CODE-2-09 (`fc42d60`) ported every dial site to
  `acquire_async` and `PooledTcp::take_stream`. The H1 take-and-discard
  pattern (in `proxy_request`) is correct but undocumented; the
  `set_reusable(false)` API exists with zero callers. The Pingora 0.8
  body-over-read class is not addressed and would re-emerge the day
  H1 upstream-reuse lands.

### ROUND8-L7-11 — `lb-h2::frame::decode_frame_low` ignores PADDED flag
- **Tag: NEW**
- Prior coverage: PROTO-2 inventory examined H2 wire-level (PROTO-2-07
  hop-by-hop, PROTO-2-11 GOAWAY). Frame-decoder padding handling was
  not audited; the bug is in `lb-h2::frame`, a crate not surfaced
  in any Round-2 finding.

### ROUND8-L7-12 — N independent H2 abuse counters, no consolidated glitches threshold
- **Tag: NEW** (quality-of-implementation, not defect)
- Prior coverage: none. `h2_security.rs` was added later in Round 4
  (six detectors). No prior audit examined operator UX of the
  cumulative threshold.

### ROUND8-L7-13 — No URI / path normalisation
- **Tag: NEW**
- Prior coverage: none. PROTO-2-15 (SNI/Host) handles authority but
  not path. Envoy edge `normalize_path` + `merge_slashes` +
  `path_with_escaped_slashes_action` were never on the docket.

### ROUND8-L7-14 — Proptest / fuzz harness does not seed CVE-class shapes
- **Tag: MISSED** (CODE-2-11)
- Prior coverage: CODE-2-11 (`560c1c2`) shipped proptest harnesses
  at `lb-h1/lb-h2/lb-h3/lb-quic` + a loom test at
  `lb-balancer/tests/loom_atomic_counter.rs`. The harnesses check
  "no panic + bounded consumption" but not "rejects ill-formed". The
  meta-finding that proptest seeds the exact CVE corpus (empty name,
  sign-prefix, oversize hex, PADDED) is exactly what CODE-2-11
  shipped *without*.

### ROUND8-L7-15 — Edge-defaults parity gap
- **Tag: NEW**
- Prior coverage: PROTO-2-14 (`tls13_only` default) + REL-2-04
  (probe defaults) touch a couple of defaults, but the canonical
  Envoy+nginx edge baseline table (max_concurrent_streams,
  initial windows, keepalive count cap, etc.) was not audited.

## L4 (12 findings)

### ROUND8-L4-01 — Backend-index 0 / zero-IP is a valid backend (Katran lesson 10)
- **Tag: NEW**
- Prior coverage: SEC-2-02 / EBPF-2-03 swapped HASH→LRU but did not
  examine the value sentinel. CODE-2-07 (Pod constructor padding)
  guards against unread `pad` bytes but allows a fully-zero
  `BackendEntry` to be live.

### ROUND8-L4-02 — Pure LRU conntrack with no TCP state awareness
- **Tag: MISSED** (SEC-2-02 / EBPF-2-03)
- Prior coverage: SEC-2-02 (`c009219`) replaced `HashMap` with
  `LruHashMap`. The Round-5 verifier in `audit/code/round-5-verifies-sec.md`
  says "LRU declaration confirmed; kernel-side eviction not
  sandbox-testable." Nobody walked back up to the Cilium `conntrack.h`
  state-machine question: LRU solves the ENOMEM-on-fill bug; it does
  NOT solve replay-old-segment evicts-live-flow.

### ROUND8-L4-03 — No SYN-flood / new-flow-rate cap on conntrack writes
- **Tag: NEW**
- Prior coverage: SEC-2-04 caps per-IP / per-listener *connections*,
  not *new flows per second*. Katran's `is_under_flood()` primitive
  is a different lever (BPF-side, throwaway-flow throttle) and was
  never audited.

### ROUND8-L4-04 — Non-atomic backend-table updates (Unimog daisy-chain)
- **Tag: NEW**
- Prior coverage: there is no single "backend table" map today —
  closest analogue is per-flow CONNTRACK. Hot-swap / atomic
  publication / daisy-chain was never on the docket. CODE-2-14
  (lb-balancer ↔ lb-core counter sync) is about *userspace*
  counter snapshots, a different layer.

### ROUND8-L4-05 — Drv→Skb attach fallback never runtime-probes XDP_TX
- **Tag: MISSED** (EBPF-2-04)
- Prior coverage: EBPF-2-04 (`75d4740`) introduced the Drv-Skb
  fallback ladder. The fallback trigger is `EOPNOTSUPP`/`EINVAL`
  from the attach syscall — Round-2 stopped at "attach mode is
  reported". Aya #1193 / MLX5+CX6 silent-drop (success on attach,
  zero packets actually flow) requires a post-attach probe that
  was not in scope.

### ROUND8-L4-06 — `insert_acl_deny` accepts `prefix_len = 0`
- **Tag: NEW**
- Prior coverage: EBPF-2-09 (Pod padding parity) touched the userspace
  loader struct fields. ACL admission gate / LPM-trie `/0` rejection
  (Cilium D4) was not audited.

### ROUND8-L4-07 — `BackendEntry::flags` is dead code; doc lies
- **Tag: NEW**
- Prior coverage: EBPF-2-09 examined `pad` bytes; `flags` was not
  touched. The doc-vs-code lie about "bit 0 means rewrite and transmit"
  is exactly the kind of doc-drift REL-2-01 / REL-2-14 worked on,
  but those rounds only checked operator docs (README/DEPLOYMENT/
  RUNBOOK), not in-source doc comments on `BackendEntry` fields.

### ROUND8-L4-08 — IPv4 fragments / IPv6 Fragment Header forwarded as if complete
- **Tag: NEW**
- Prior coverage: none. Pillar 4a/4b non-goals are documented in
  ADRs (per references) but not enforced in the BPF program. No
  Round-2 finding on fragment handling.

### ROUND8-L4-09 — `ptr_at` bounds check vulnerable to aya #1562 / no overflow guard
- **Tag: MISSED** (EBPF-2-07)
- Prior coverage: EBPF-2-07 was supposed to ship verifier-log capture
  so we could detect aya #1562-class operand-reordering rejects. The
  capture infra (`scripts/verify-xdp.sh`) is committed but the .log
  baselines aren't (see ROUND8-L4-10). `ptr_at`'s overflow guard
  on `start + offset + len` was never explicitly checked.

### ROUND8-L4-10 — EBPF-2-07 is "Verified-Fixed" but no verifier logs committed
- **Tag: DISPUTE** (EBPF-2-07)
- Prior verdict: EBPF-2-07 marked `Verified-Fixed(ffde98c)` by `rel`
  round-5; `ebpf` rerun + `proto` Round-7 final report retained the
  verdict.
- Round-8 view: `git show ffde98c --stat` returns only
  `scripts/verify-xdp.sh` (+109 lines) and
  `audit/ebpf/verifier-logs/README.md` (+30 lines). No `.log` files
  are committed; the diff-gate at `verify-xdp.sh:111` is a permanent
  no-op (the `.committed` file never exists). `audit/unsafe-justifications.md:109`
  asserts the gate is live — false.
- This goes into `disputes.md` for lead arbitration.

### ROUND8-L4-11 — Loader does not enforce `/sys/fs/bpf` mount-type before pinning
- **Tag: MISSED** (EBPF-2-05)
- Prior coverage: EBPF-2-05 (`37c513c`) wired map pinning across
  restarts. The Round-5 verifier note says "Bpffs-missing failure
  mode confirmed as loud-fail via `XdpLoaderError::Load`." That's
  about the *load* path, not the *pin* path's mount-type check; the
  doc comment at `DEPLOYMENT.md` says "caller is responsible" — which
  is exactly the gap.

### ROUND8-L4-12 — XDP attach does not use `BPF_F_REPLACE`
- **Tag: MISSED** (EBPF-2-04, CODE-2-10)
- Prior coverage: EBPF-2-04 ladders Drv→Skb fallback. CODE-2-10
  (`854ebdb`) added drop-semantics tests. Multi-program attach
  coexistence — `bpf_xdp_query` + `BPF_F_REPLACE` semantics — was
  not in scope. The handoff cross-cutting item 1(c) explicitly
  asked for `BPF_F_REPLACE`; the previous rounds graded the fallback
  ladder as Verified-Fixed but did not check the multi-program path.

## OPS (12 findings)

### ROUND8-OPS-01 — README claims FD-passing; not implemented
- **Tag: MISSED** (REL-2-01, REL-2-14)
- Prior coverage: REL-2-01 (`f2bf64c`) did a doc rewrite, but the
  specific README line "Zero-downtime reload via SO_REUSEPORT and FD
  passing" survived. REL-2-14 deleted `/usr/local/bin/lb` references
  but not aspirational capability claims. The `CONFIG.md:136` deferral
  note exists; the README claim contradicting it does too.

### ROUND8-OPS-02 — Drain has no jitter / randomisation
- **Tag: NEW**
- Prior coverage: REL-2-02 spec'd drain ordering + budget but did
  not address probabilistic close-distribution (Envoy
  `drain_manager_impl.cc`). The jitter primitive is operationally
  invisible in single-instance tests, so Round-2 missed it not
  because they looked and stopped short but because the bug shape
  is in multi-replica deploys nobody simulated.

### ROUND8-OPS-03 — `shutdown_drain_seconds` histogram never landed
- **Tag: MISSED** (REL-2-02)
- Prior coverage: REL-2-02 recommendation line 134 explicitly required
  "shutdown_drain_seconds histogram + shutdown_aborted_connections_total
  counter". The counter shipped; the histogram did not. Round-7
  graded REL-2-02 `Verified-Fixed`. `grep shutdown_drain_seconds`
  across `crates/` returns zero hits. Spec-vs-implementation gap.

### ROUND8-OPS-04 — TCP listener accept loop has no cancel arm
- **Tag: MISSED** (CODE-2-03)
- Prior coverage: CODE-2-03 (`9ff2b9b`, `fc050b0`, `bca4285`) marked
  `Verified-Fixed`. The status note claims "listener accept loop"
  was migrated to `shutdown.tracker().spawn(...)`. Spawning through
  the tracker is correct, but the **accept loop itself** at
  `crates/lb/src/main.rs:2180` still does `listener.accept().await`
  with no `tokio::select!` cancel arm. Verified by reading the file:
  loop has classify_accept_error / next_accept_backoff but no
  cancel. The drain at line 1942-1944 uses `JoinHandle::abort()` —
  which CODE-2-03 was supposed to retire.

### ROUND8-OPS-05 — `LabelBudget::check` startup-only, no per-emission gate
- **Tag: MISSED** (REL-2-08)
- Prior coverage: REL-2-08 (`551d470`) marked Verified-Fixed-Partial.
  The "Partial" was because emit sites use only ["version","status_class"]
  and not the full canonical schema. The cardinality gate is *also*
  partial — startup-only Cartesian-worst-case math, no runtime
  re-evaluation when `route` label population begins. Round-8 escalates
  what Round-7 acknowledged as Partial.

### ROUND8-OPS-06 — Tracing context propagation library exists, zero L7 callsites
- **Tag: MISSED** (REL-2-07)
- Prior coverage: REL-2-07 (`1d462c7`) marked Verified-Fixed-Partial.
  Status note already discloses: "Library-only fix; proxy wire-in is
  deferred." Round-7 final report retained Partial. Round-8 stance is
  this is still a divergence and severity should escalate (operator
  visibility on distributed traces is the bar three references converge on).

### ROUND8-OPS-07 — Systemd unit missing modern hardening directives
- **Tag: MISSED** (SEC-2-11, REL-2-01)
- Prior coverage: SEC-2-11 (`e44117d`) closed the `CAP_SYS_ADMIN`
  fallback. REL-2-01 (`f2bf64c`) updated `DEPLOYMENT.md`. Neither
  scored the unit against `systemd-analyze security`. Missing knobs:
  `SystemCallFilter=@system-service`, `RestrictAddressFamilies`,
  `ProtectKernel*`, `RestrictNamespaces`, `PrivateUsers`, etc. Also:
  the unit is documented as a code block, not a committed
  `packaging/expressgateway.service` file — no CI gate possible.

### ROUND8-OPS-08 — `audit/sbom.json` generated by `manual-fallback` tool
- **Tag: NEW**
- Prior coverage: SEC-2-07 hardened CI with `cargo-audit -D warnings`
  + `cargo-geiger` + `cargo-machete`. SBOM provenance was deferred to
  CI and never closed. The current file declares
  `"tools": [{ "vendor": "manual-fallback" }]` — non-conformant
  CycloneDX provenance.

### ROUND8-OPS-09 — `doc-lint.sh` too narrow; misses claim drift
- **Tag: MISSED** (REL-2-01, REL-2-14)
- Prior coverage: REL-2-01 / REL-2-14 ratcheted doc-lint patterns
  for the binary-name bug class. Six items in the original REL-2-01
  drift list; only one (binary name) became a CI gate. The
  doc-lint *philosophy* — treat every operator-doc number as a
  contract — was not adopted.

### ROUND8-OPS-10 — Drain budget 10 s below Pingora norm for long-lived streams
- **Tag: NEW**
- Prior coverage: REL-2-02 spec'd the drain budget but at a
  per-process scalar (default 10 000 ms). No per-listener override,
  no `RUNBOOK.md` guidance for streaming workloads. Pingora's 300-s
  `EXIT_TIMEOUT` reference was not previously cited.

### ROUND8-OPS-11 — `/readyz` flip-to-503 has no inflight grace inside probe scrape window
- **Tag: MISSED** (REL-2-04)
- Prior coverage: REL-2-04 (`7108d9e`) shipped `/livez+/readyz+/startupz`
  + `set_draining()` flip. The `readiness_settle_ms=1000` value was
  not interrogated against the kubelet probe-period (10 s default).
  Single-instance test setup masked the interaction.

### ROUND8-OPS-12 — Container image lacks RO rootfs + healthcheck + LABEL provenance
- **Tag: NEW**
- Prior coverage: REL-2-01 / REL-2-14 mentioned the binary path
  inside the Dockerfile but did not audit OCI labels, HEALTHCHECK,
  or `securityContext.readOnlyRootFilesystem`. Distroless+nonroot
  was treated as sufficient.

---

## Tag counts

| Tag | Count |
|---|---|
| NEW       | 24 |
| MISSED    | 14 |
| DISPUTE   | 1 (ROUND8-L4-10) |
| **Total** | **39** |

## MISSED detail

14 of 39 = **35.9%** MISSED.

The escalation rule is ">10% MISSED → systematic blind spot". We are
**3.6x** the threshold. See `coverage-gap.md`.
