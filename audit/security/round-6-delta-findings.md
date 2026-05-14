# Round 6 — sec delta-discovery sweep on `prod-readiness/round-4`

Scope: diff between `main` and `prod-readiness/round-4` (d1e1247) — looking
for **new** attack surface introduced by the Wave-2 fixes. Prior-round
findings are explicitly out of scope; only deltas land here.

## Methodology

* Read every new / heavily-rewritten file in `crates/lb-security/`,
  `crates/lb-observability/admin_http.rs`, `crates/lb-l7/security_hooks.rs`,
  and the lifecycle / cert-rotation glue in `crates/lb/src/main.rs`.
* Cross-checked the SecurityHooks → HooksBundle → ConnGate / Watchdog API
  surface for reflection, trait-object soundness, and Drop-time hazards
  under `panic = "abort"`.
* Reviewed `ArcSwap<TlsConfigBundle>` rotation under SIGUSR1, including
  the partial-write window during a cert push.
* Reviewed `AdminAuthGate::validate_bind` for IPv6-mapped IPv4, link-local,
  and unspecified-address bypass; reviewed token comparison for
  constant-time semantics.
* Reviewed `/metrics`, `xdp_metrics`, and `stats_export` for label
  cardinality / value-poisoning surface.
* Reviewed Cargo.lock delta for newly-added runtime dependency CVEs
  (object, notify, rcgen, opentelemetry, aya, dashmap, arc-swap, subtle).
  `cargo-audit` is not installed in this environment; CVE check is
  version-based against published advisories as of the assistant
  knowledge cutoff.

## Result

**Zero new medium-or-higher findings.** The Round-4 fix surface holds
under the seven attack vectors enumerated in the Round-6 brief.

Specifically:

* **Cert rotation (REL-2-03)** — `reload_tls_bundle` fully constructs the
  new `TlsConfigBundle` (parse + rustls smoke-build + ALPN + ticketer)
  before calling `ArcSwap::store`, and stores an `Arc<TlsConfigBundle>`
  in a single atomic pointer swap. No path can publish a half-loaded
  bundle, even under a partial-write race where SIGUSR1 fires between
  cert and key being written: a partial chain or key triggers a typed
  `TlsBundleError` (`EmptyChain` / `NoKey` / `KeyMismatch`) before
  `store` runs, the old bundle stays live, and the failure increments
  `cert_rotation_failed_total{reason=…}`. Per-accept readers continue to
  observe the old bundle until the swap commits.

* **Shutdown drain ordering (REL-2-02)** — `set_draining()` flips probe
  state to `Draining` *before* the `settle_ms` sleep, so `/readyz`
  returns 503 the moment the signal handler runs. The `shutdown.token()
  .cancel()` call follows the settle window and explicitly precedes the
  listener `.abort()` loop. Requests that land during settle are
  accepted by design (upstream LB drain window) and complete inside the
  drain budget; there is no TOCTOU between `set_draining` and `cancel`
  that lets `/readyz` flap back to 200.

* **Admin auth (SEC-2-06)** — `validate_bind` correctly rejects
  `0.0.0.0`, `[::]`, and non-loopback IPv4/IPv6 binds without an
  explicit `allow_non_loopback = true` *and* an `api_token_hash`.
  `Ipv4Addr::is_loopback()` is 127.0.0.0/8, `Ipv6Addr::is_loopback()` is
  exactly `::1`; the IPv4-mapped IPv6 form `::ffff:127.0.0.1` is
  **not** considered loopback by stdlib and therefore goes through the
  override+token branch (safe-by-default — there is no implicit bypass).
  Bearer token comparison goes through `subtle::ConstantTimeEq` on the
  full 32-byte SHA-256 digest after a constant-cost hash; no length or
  prefix-based timing oracle is reachable.

* **STATS / metrics poisoning** — all `with_label_values(&[…])` call
  sites use compile-time-bounded label vocabularies (listener name from
  config, `version` ∈ {"HTTP/1.0", "HTTP/1.1", "HTTP/2", "HTTP/3"},
  `status_class` ∈ {"2xx","5xx"}, XDP slot names from
  `stat_slot_labels()`). The XDP per-slot reader sums the per-CPU array
  with `wrapping_add`, which is the expected semantics for monotonic
  counters; no untrusted input feeds the slot index or the bytes
  summed. STATS values originate from the kernel-side XDP program;
  there is no client-writable path into the map.

* **`panic = "abort"` cleanup** — the panic hook explicitly calls
  `std::process::abort()` after a best-effort tracing flush. No
  reviewed Round-4 module relies on unwind for cleanup of
  file-descriptor, lock, mmap, or pidfile state outside its `Drop`
  impl. `ConnPermit::Drop` carries a `debug_assert!` that under abort
  cannot double-panic. Tickets / TLS bundles are reference-counted; the
  abort drops the whole address space at once.

* **Newly-added runtime crates** (versions in lockfile):
  `arc-swap 1.9.1`, `aya 0.13.1`, `dashmap 6.1.0` (and transitive
  `dashmap 5.5.3` via `governor → foundations → tokio-quiche`),
  `object 0.36.7 / 0.37.3`, `opentelemetry 0.22.0`,
  `rcgen 0.13.2`, `subtle 2.6.1`. None of these versions match an open
  Rust advisory database entry as of the cutoff. `dashmap 5.5.3`
  contains the fix for RUSTSEC-2023-0040. The only Cargo.lock entries
  *introduced* by this branch (`fastrand`, `loom`, `proptest`,
  `rustix`, `tempfile`, `wait-timeout`, etc.) are `[dev-dependencies]`
  pulled in by Round-4 test additions; they do not ship in the release
  binary.

## Informational observations (sub-low; recorded for completeness, not regressed)

These do not loop back to Round 3 but are listed so a future hardening
pass can pick them up.

### INFO-DELTA-1 — `to_str().unwrap_or("")` in `HooksBundle::inspect_request`
File: `crates/lb-security/src/hooks.rs:135`
Severity: info
Introduced-by: e36b50f
The header→pairs conversion silently substitutes `""` when a
`HeaderValue` is not valid ASCII (`to_str()` returns `Err`). The
explicit defense-in-depth `SmuggleDetector::check_all_mode` call in
`lb-l7/h1_proxy.rs:550` uses the same pattern with `filter_map`, which
drops the header outright. Real-world impact is gated by hyper's H1
parser, which already rejects high-bit bytes in `Transfer-Encoding` and
`Content-Length` headers, so a wire-level value containing `0x80..0xff`
never reaches the detector. Recommendation: switch to
`HeaderValue::as_bytes()` and let the detector reason over raw bytes —
this removes one assumption about the upstream parser's strictness.

### INFO-DELTA-2 — `Watchdog::register` cap is racy
File: `crates/lb-security/src/watchdog.rs:187-201`
Severity: info
Introduced-by: 1f7f417
`len() >= max_registered` check and the subsequent `insert` are not
atomic. N concurrent `register` calls passing the check together can
each insert, exceeding the cap by at most N-1. Overhead per excess
entry is one DashMap slot (~64 B); the documented 100 000-entry ceiling
implies ~6 MB peak slop under maximum concurrent admit. Not a security
boundary — listener-cap accounting is owned by `ConnGate` which **does**
gate atomically. Recommendation: switch to a `compare_exchange` on a
sibling `AtomicUsize` counter, mirroring the `ConnGate::admit` pattern.

### INFO-DELTA-3 — `/livez` / `/readyz` / `/startupz` / `/healthz` bypass admin bearer auth
File: `crates/lb-observability/src/admin_http.rs:68-94`
Severity: info
Introduced-by: 9484544
By design, probe endpoints are exempt from the SEC-2-06 bearer-token
gate even when bearer enforcement is active. The exemption is
documented and intentional (kubelet anonymous probes). The body the
probe returns is the closed-set `ProbeState::body_token()` —
`{"status":"starting|ready|draining|stopped"}` — which leaks the
gateway lifecycle phase to anyone who can reach the admin port. With
the safe-by-default `validate_bind` posture this is loopback-only; with
`allow_non_loopback = true` the exposed surface is one of four
states. Recommendation: document this explicitly in `SECURITY.md` under
the "admin listener" header so operators choosing `allow_non_loopback`
understand what the probe surface still discloses.

### INFO-DELTA-4 — `ConnGate::admit` may leave a `0`-valued per-IP entry on cap=0
File: `crates/lb-security/src/conn_gate.rs:195-201`
Severity: info
Introduced-by: e36b50f
If `per_ip_cap == 0`, `entry().or_insert(0)` materialises a fresh `0`
entry, the `*entry >= per_ip_cap` check trips, and the entry is
released without being removed. The listener-counter rollback runs
correctly; the residual `0` entry is GC'd by the next `admit` that
*succeeds* on that peer (decrement-to-zero on permit drop calls
`remove_if`). Pathological config (`per_ip_cap = 0`) is functionally
"reject all" so the leak only matters as a memory-pressure vector if an
operator misconfigures the cap to zero in production. Recommendation:
short-circuit the check before the DashMap `entry()` call when
`per_ip_cap == 0`.
