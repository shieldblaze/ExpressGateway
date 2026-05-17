# Lead Decisions Log

Plan-approval decisions during Round 3, and any other lead arbitrations
that surface mid-round. Each entry is one paragraph + verdict.

## L-001 — SEC-2-14: remove `lb-compression` crate (Option A)

**Question (from sec, Round 3):** sec proposed two options for the
unused `Decompressor`: (A) remove the entire `lb-compression` crate,
(B) wire it into the H1/H2 proxy as a body-inspection filter.

**Decision: Option A (approve removal).** The crate has zero in-workspace
consumers outside its own tests; the proxy is pass-through and does not
decompress in either direction. The README claim
"Compression: zstd, brotli, gzip, deflate with streaming and
transcoding" is aspirational — fold the correction into `REL-2-01`'s
doc-lint table-of-truth. Re-introduction (BREACH guard, gRPC
max-message-size, JSON inspection) is on the deferred list with a
short reintroduction note in `audit/deferred.md`.

**Code action:** code's `CODE-2-15` plan picks up the workspace
`[members]` removal; sec writes the ADR; rel updates the README + docs.

## L-002 — SEC-2-99-A: per-IP cap vs CGNAT egress

**Question (from sec, Round 3):** `per_ip_cap = 256` will trip
legitimate clients sharing one egress NAT (corporate CGNAT). Mitigation
idea: `[security].trusted_cidrs` allowlist with per-CIDR elevated caps.

**Decision: defer to `audit/deferred.md`** as a Round-4 follow-up
enhancement. The minimum-viable fix (per-IP cap) lands in Round 4 as
designed under SEC-2-04. The trusted-CIDR overlay is an enhancement,
not a defect — it requires a config-schema extension and operator-side
discovery model. Lead's pre-acknowledgement of this deferral: yes —
user will see it in the deferred register at FINAL.

**Operator-side guidance:** RUNBOOK.md gains a "Tuning the per-IP cap"
section covering this concrete scenario (rel's REL-2-01 covers).

## L-003 — Pre-recorded from Round-2 synthesis §E (acknowledgements)

For completeness, the five Round-2 decisions captured in
`CROSS-REVIEW-SYNTHESIS-r2.md` §E are also lead-decisions and should
appear here as one-liners:

1. **PROTO-2-07** newtype `StrippedRequest` — APPROVED.
2. **PROTO-2-14** TLS 1.2 — KEEP DEFAULT; add knob only.
3. **CODE-2-04** one umbrella plan with per-site appendix — APPROVED.
4. **CODE-2-09** → `tokio::net::TcpStream::connect` — APPROVED.
5. **CODE-2-13** + **REL-2-05** wiring scoped to file-backed CP + passive health — APPROVED; distributed CP backends DEFERRED.

## L-004 — Plan word-cap overruns on CODE-2-04 / CODE-2-11

The 500-word cap on plans was a lead-imposed style guideline, not a
hard limit. CODE-2-04 (atomic-ordering audit, ~755 words) includes the
lead-authorised per-site appendix. CODE-2-11 (proptest/loom/miri,
~846 words) enumerates 12 proptest harnesses with budgets per the
audit-gate spec. Both are justified; **approved as written**.

## L-005 — EBPF-2-01 + EBPF-2-02 plan merge

ebpf merged the license + BTF + linker-section work into a single
plan (`audit/ebpf/plans/EBPF-2-01.md`) with EBPF-2-02.md as a merge
pointer. **APPROVED** — same files, same proof test.

## L-007 — Bulk plan approval (Round 3 → Round 4)

All 55 plan files under `audit/*/plans/` were format-checked
(Finding-ref, Files-touched, Approach, Proof, Risk, Cross-ref present;
batch-lows use per-item paragraphs which is acceptable). The 6 critical
plans were content-spot-read (CODE-2-01/03/05, REL-2-02/03, PROTO-2-02,
PROTO-2-11, SEC-2-01) — each names exact file paths, concrete code
sketches, and runnable proof tests with named invariants.

**Decision: bulk approve all 55 plans.** Lead-approval line in every
plan file updated to `approved 2026-05-13 team-lead`.

Minor notes recorded for Round 4 implementers:
- **PROTO-2-02** flagged a §D allocation mismatch: synthesis listed
  `crates/lb-quic/src/config.rs` but the live
  `set_application_protos` is in `crates/lb-quic/src/lib.rs`. proto's
  plan handles either keep-or-relocate. Round-4 owner decides
  locally — no lead action.
- **PROTO-2-10** carries the corrected sec-§A.7 hyper-coverage matrix
  (sec confirmed in cross-review that hyper 1.9.0 catches ~70% of
  CL/TE smuggling variants; one row correction owed when sec's
  SEC-2-15 matrix and proto's PROTO-2-10 plan land together).
- **CODE-2-08** drop-guard is dead code under `panic = "abort"`
  (CODE-2-02). This is correct and intentional; the guard exists
  for the dev/test profile which still uses `unwind`.

## L-006 — `stats_export.rs` API boundary (EBPF-2-08 ↔ REL-2-13)

ebpf creates `crates/lb-l4-xdp/src/stats_export.rs` as the
public-API boundary; rel calls into it from `lb-observability` for
Prom export. Neither edits the other's files. **APPROVED** — this
is exactly the design baked into synthesis §D.
