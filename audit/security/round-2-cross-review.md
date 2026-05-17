## SEC — Round 2 Cross-Review

Owner: `sec` (security-reviewer). Round 2 / cross-review pass after
all five teammates published `audit/<area>/round-2-{review,findings}.md`.

Files read:
- `audit/code/round-2-review.md` (15 findings, CODE-2-01..-15)
- `audit/ebpf/round-2-review.md` (9 findings, EBPF-2-01..-09) +
  `audit/ebpf/round-2-cross-review.md`
- `audit/reliability/round-2-review.md` (15 findings, REL-2-01..-15) +
  `audit/reliability/round-2-cross-review.md`
- `audit/protocol/round-2-review.md` (15 findings, PROTO-2-01..-15)

Verdict legend: **AGREED** / **DISPUTED** / **ESCALATE-SEVERITY** /
**DOWNGRADE-SEVERITY** / **MERGE** / **OUT-OF-SCOPE-FOR-SEC**.

Only non-trivial cross-area findings are listed — pure-area items
(e.g. PROTO-2-04 ws_autobahn stub, REL-2-14 binary-name mismatch)
have no security cross-cut and are silently acknowledged.

---

## A. Smuggling / hop-by-hop / request-integrity

### A.1 PROTO-2-10 ↔ SEC-2-01 ↔ CODE-2-01 ↔ T1
Verdict: **AGREED**, **MERGE recommended** — all three findings
describe the same wiring gap from three angles.

Proto's coverage matrix in PROTO-2-10 is the canonical defense-vs-
hyper-coverage analysis. SEC-2-15 corroborates with the same
hyper-1.9.0 line-by-line analysis (independently derived) and
reaches the same ~70%-coverage estimate. Code's CODE-2-01 owns the
dependency-graph fix. No disputes.

Recommended Round 3 plan-assignment: a single workstream owned by
`code` (graph fix) + `proto` (semantic spec + integration tests) +
`sec` (audit the wired result). Severity: **high** (concur with
proto's high; my Round-1 framing as high was correct).

### A.2 PROTO-2-08 — `trailers` in HOP_BY_HOP
Verdict: **AGREED** + **ESCALATE-SEVERITY** from proto's medium to
high if combined with PROTO-2-12 (trailer pass-through broken).

Proto's claim that `trailers` (the *value* of a `Connection: TE`
header, not a header name) is being stripped as if it were a
header name is a real bug: it can never match because there is no
`Trailers:` header in HTTP. The bug is **inert** today (the strip
list runs without effect for that entry) but signals that the
author confused the connection-header *option* with a header name
— a closely related mistake (forgetting to scrub `Connection: te`
options) is a real smuggle vector. SEC concurs with proto's
recommendation and keeps severity **medium**, no escalation.

### A.3 PROTO-2-07 — Bridge trait hop-by-hop stripping
Verdict: **AGREED**, **OUT-OF-SCOPE-FOR-SEC**. Lead synthesis §B.3
defers placement decision to Round 3. SEC has no security objection
to either placement (helper-invoked or trait-level), provided the
contract is explicit and tested.

---

## B. Per-IP cap / accept-loop / DoS

### B.1 REL-2-09 ↔ CODE-2-05 ↔ SEC-2-04
Verdict: **AGREED**, all three describe the same hot-loop spawn
gap. Severity convergence: rel high, code high, sec high. No
dispute.

### B.2 REL-2-10 ↔ CODE-2-06 ↔ SEC-2-04 (EMFILE)
Verdict: **AGREED**. The EMFILE tight-loop is a real liveness
trigger and amplifies SEC-2-04. Severity: high. Combined fix
(semaphore + backoff) is one PR.

### B.3 SEC-2-03 / SEC-2-10 ↔ REL-2-02 (TLS-accept slowloris)
Verdict: **AGREED**. rel's F-05 / REL-2-02 covers the
`acceptor.accept()` timeout vector. SEC-2-10 (TLS handshake
slowloris) and SEC-2-03 (slowloris detector unwired) are sibling
findings. Joint fix.

---

## C. XDP / eBPF surface

### C.1 EBPF-2-01 ↔ SEC-2-12 (BPF license)
Verdict: **DOWNGRADE-SEVERITY** on SEC-2-12 from medium to **low**.

Ebpf did the work I did not: confirmed that **aya-obj 0.2.1's
default license string is `"GPL"`** when no `license` section is
present (`aya-obj-0.2.1/src/obj.rs:459-463`, quoted in EBPF-2-01).
Therefore today the kernel sees a `"GPL"` license string at
`BPF_PROG_LOAD` even with the missing ELF section, and the
program loads. SEC-2-12's "program load fails" impact statement is
**not currently true**. The finding is still real because:
1. The deployed binary is correctly licensed but **opaque** —
   anyone reading the ELF cannot verify the license claim without
   reading aya-obj source code.
2. Any future aya minor version that changes the default would
   silently break attach (SEV-1 startup).

SEC-2-12 is hereby revised in spirit to **medium → low**:
hardening / future-proofing, not a current-state failure. Ebpf's
recommendation (add the explicit `link_section = "license"` static)
is the correct fix.

### C.2 EBPF-2-09 ↔ CODE-2-07 ↔ SEC-2-09 (Pod padding)
Verdict: **AGREED** on the latent-hazard framing.

Ebpf's EBPF-2-09 confirms my Round-1 finding that the kernel-side
BPF program reads the keys via `bpf_map_lookup_elem` and **does
not** mask out the `pad` bytes; the verifier permits the full key
to participate in the hash. Code's CODE-2-07 takes ownership of
the constructor / `Default` impl fix.

SEC-2-09 severity remains **low** today, with explicit re-promote
trigger documented (control-plane wiring landing). I concur with
code's CODE-2-07 framing of severity high in the after-wiring
world.

### C.3 EBPF-2-03 ↔ REL-2-12 ↔ SEC-2-02 (CONNTRACK LRU)
Verdict: **AGREED**, no severity dispute. ebpf high, rel high,
sec high. Tri-owner Round 3 work.

### C.4 EBPF-2-05 ↔ REL-2-03 (map pinning + cert rotation)
Verdict: **AGREED** on EBPF-2-05; **OUT-OF-SCOPE-FOR-SEC** on the
bpffs perms recommendation (rel cross-review §126-133 asked sec to
provide a bpffs-perms finding). My answer: the right mode is
`0o750` with `root:expressgateway-bpf` ownership, pin path
`/sys/fs/bpf/expressgateway/`. This is hardening-grade, not a
discrete finding — sec accepts it as a sub-recommendation of
EBPF-2-05.

### C.5 EBPF-2-04 (SKB-only, no Drv probe)
Verdict: **AGREED**. No security cross-cut (production-readiness
issue). No-op for sec.

### C.6 EBPF-2-07 (verifier-log matrix)
Verdict: **AGREED**, **OUT-OF-SCOPE-FOR-SEC**. Process item.

---

## D. Atomic ordering

### D.1 CODE-2-04 ↔ SEC-2-16
Verdict: **AGREED**. Code's appendix-A per-site table is exactly
what SEC-2-16 hands back to code, with the same classifications.
The two items I left as `R` (review-required) — `quic_pool.rs`
gates — SEC concurs are `G` (enforcement gates) and should be
`AcqRel`/`Acquire`.

Per the detector list code deferred to sec:
- `RapidResetDetector` — `&mut self` plain struct, no atomic
  needed. **No change.**
- `ContinuationFloodDetector` — same. **No change.**
- `HpackBombDetector` — same. **No change.**
- `ZeroRttReplayGuard` — uses `parking_lot::Mutex<HashSet>`, no
  atomic ring. **No change.** Code's "SeqCst on ring CAS" is
  predicated on a future redesign (e.g. lock-free Bloom filter
  per SEC-2-05); when that lands, SeqCst is correct.
- per-IP rate-limit bucket — when SEC-2-04 wiring lands, the
  bucket counter is a `G` enforcement gate requiring `AcqRel`
  on the CAS.

Severity: code high, sec info. SEC defers severity to code's high
because reordering on weak-memory hardware is a real bug — the
fact that x86-64 hides it does not make it not-a-bug.

---

## E. SIGTERM / drain / cert rotation

### E.1 REL-2-02 ↔ CODE-2-03 ↔ PROTO-2-11 ↔ T2
Verdict: **AGREED**, all three describe the same SIGTERM gap.
Severity convergence: rel high, code high, proto high. No
dispute.

Security cross-cut: a non-graceful SIGTERM during a TLS handshake
leaves the rustls session in a partially-decrypted state on the
peer side. No secret material leaks (rustls zeroizes on drop), but
the half-open state allows the next operator-initiated connection
to inherit the listener's `SO_REUSEPORT` queue and receive a
malformed first record from the peer — a low-severity
fingerprinting vector, not a vulnerability. SEC raises no
additional finding.

### E.2 REL-2-03 ↔ SEC-2 (cert rotation)
Verdict: **AGREED**. SEC-2 (my Round-1 §5.1) merges into REL-2-03;
rel is the owner. No separate sec finding.

### E.3 CODE-2-02 (`panic = "abort"` not set) ↔ sec posture
Verdict: **ESCALATE-SEVERITY** from code's high to **high
(unchanged)** with security cross-cut documented.

A panic mid-`unsafe` block in `crates/lb-l4-xdp/src/loader.rs` or
`crates/lb-io/src/ring.rs` (71 `unsafe` sites total per my Round-1
§3) under unwind can leave half-initialized BPF map entries or
half-submitted io_uring SQEs visible to the kernel. Worst case: a
kernel-side use-after-free if the SQE points at userspace memory
that the Rust unwinder drops mid-submission. SEC concurs with
code's "set `panic = abort`" recommendation as a **security
hardening** measure, not just reliability.

---

## F. Admin endpoint / observability / metrics

### F.1 REL-2-04 ↔ SEC-2-06 (admin authn + readyz)
Verdict: **AGREED**. Joint sec + rel. Severity convergence:
medium / medium.

### F.2 REL-2-06 ↔ no sec cross-cut (JSON logs)
Verdict: **AGREED**, no sec security cross-cut, except: SEC asks
that the JSON-log decision include a hard rule that **never log
request/response bodies, `Authorization`, or `Cookie` values at
any level**. rel's recommendation does not explicitly state this;
sec will track in the Round 3 plan.

### F.3 REL-2-07 (traceparent) — no sec cross-cut
Verdict: **OUT-OF-SCOPE-FOR-SEC**. Note: `traceparent` is a
trust-on-first-write header; if the gateway is internet-facing,
naive forwarding without scrubbing allows trace-context
poisoning. Recommend the trace propagation implementation include
"strip-and-mint" mode for ingress and "pass-through" mode for
peer-trusted. Sec defers to rel for the placement decision.

### F.4 EBPF-2-08 / REL-2-13 (STATS export) — no sec cross-cut
Verdict: **OUT-OF-SCOPE-FOR-SEC**.

---

## G. Protocol correctness

### G.1 PROTO-2-01 (`:authority` vs `Host`)
Verdict: **AGREED**, no severity dispute. Security cross-cut:
disagreement is the same shape as smuggling — gateway and
upstream disagree on the request target. Sec considers this a
**high-severity** finding (proto rated high). Joint with
SEC-2-01.

### G.2 PROTO-2-02 (`LB_QUIC_ALPN = b"lb-quic"`)
Verdict: **AGREED**. Security cross-cut: incorrect ALPN means no
production H3 listener exists, so QUIC-side mitigations
(`RetryTokenSigner`, `ZeroRttReplayGuard`) are not actually
defending production HTTP/3 traffic — they defend a `lb-quic`
private protocol that no client speaks. This is a deployment
disconnect, not a vulnerability per se, but it means the QUIC
attack surface review (my SEC-2-05) is largely theoretical
until the ALPN is fixed.

### G.3 PROTO-2-14 (TLS 1.2 enabled, no `tls13_only`)
Verdict: **AGREED**, **DOWNGRADE-SEVERITY** from proto's medium to
low. TLS 1.2 with rustls 0.23.38 + ring is configured with safe
defaults (no SSLv3, no CBC suites in rustls' default policy,
secure renegotiation off). The "TLS 1.3 only" lever is a
compliance / posture lever, not a security gap. Sec recommends
the config option be added but at low priority.

### G.4 PROTO-2-15 (SNI ↔ Host mismatch unenforced)
Verdict: **AGREED**, **ESCALATE-SEVERITY** from proto's medium to
**high** when combined with SEC-2-01 (smuggle) or PROTO-2-01
(host disagreement). A client sending `SNI = a.example` and
`Host: b.example` can route to backend group A but be matched
against backend group B's auth policy; with no enforcement the
gateway opens a virtual-host confusion vector. Sec recommends
high.

### G.5 PROTO-2-12 (trailer pass-through broken)
Verdict: **AGREED**. Security cross-cut: a malicious upstream
returning trailers with `Trailer: Transfer-Encoding, X-Foo` could
re-introduce a CL/TE smuggle on the gateway↔client hop if the
gateway forwards trailers verbatim into an H1 client connection.
Sec asks proto to add this case to their integration tests.

### G.6 PROTO-2-09 (silent protocol fall-through)
Verdict: **AGREED**, **ESCALATE-SEVERITY** from proto's medium to
**high**.

A typo in `protocol = "h1s"` → `protocol = "h1S"` silently produces
a `PlainTcp` listener, **stripping all TLS** from a listener the
operator believes is TLS-protected. This is a clean **TLS
downgrade by misconfig**, which is exactly the operator-error
class my Round-1 §2.1 called "operator misconfiguration (high
frequency)". Sec recommends:
- Hard-error on unknown `protocol = …` values (proto's
  recommendation; concur).
- Validation-pass before the listener spawns (recommend
  `parse_config` calls a `validate_protocol_strings` helper that
  errors with a list of valid values).
Severity: high.

---

## H. Items I open / escalate

### H.1 New finding from cross-review: **operator-error TLS
downgrade chain** (SEC-2-09→SEC-2-15 weren't enough)

Combining PROTO-2-09 (silent fall-through) + SEC-2-08 (no
key-mode assertion) + SEC-2-06 (admin endpoint operator-trust):
the *operator-error attack surface* is large enough that the
config-parse pass should be hardened into a **strict-mode**
default. Sec proposes a Round 3 plan item: a new top-level config
`[runtime].strict_mode = true` (default) that:
- Refuses non-loopback `metrics_bind` without `unsafe_bind_public`.
- Refuses unknown protocol strings.
- Refuses TLS key files with mode > 0o600.
- Refuses `H2SecurityThresholds` larger than 4× defaults without
  `unsafe_override = true`.

This is not a discrete Round-2 finding (it's a Round-3 plan
proposal), but recorded here so the team-lead sees the unified
hardening posture.

---

## I. Disagreements requiring lead resolution

None. The Round-1 lead-flagged disagreements all resolved:

1. **0-RTT on TCP listener (rel F-19)**: **WITHDRAWN** —
   SEC-2-13 documents the resolution. `build_server_config`
   never sets `max_early_data_size`, rustls 0.23.38 defaults
   to 0. Rel did not re-open in Round 2; consistent.

2. **Drain budget**: lead-pre-emptive decision (10 s, hard cap).
   rel REL-2-02 adopts this. No sec objection.

3. **Bridge hop-by-hop placement**: deferred to Round 3 plan.
   PROTO-2-07 surfaces both options. Sec out-of-scope.

4. **`spawn_blocking` for upstream connect**: rel REL-2-11
   authored; code CODE-2-09 evaluated. Both findings agree
   (high severity). Sec defers to rel/code joint owner.

---

## J. Items blocking Round 3 plan-assignment

None from sec's perspective.

---

— `sec`, Round 2 cross-review.
