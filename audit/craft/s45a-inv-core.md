# scanner-core — lb / lb-l4-xdp / lb-soak

## Summary

files_scanned=65  slop=3  ambiguous=20  load_bearing_notable=24

Total comment units inventoried: 1903 (contiguous comment blocks + trailing inline comments).
Load-bearing after subtracting slop+ambiguous: 1880 (98.8%).

### `deny(missing_docs)` gate rule — applied, counts UNCHANGED

Lead correction applied: a `///` / `//!` on a `pub` item under `#![deny(missing_docs)]` is
GATE-LOAD-BEARING (removal turns `clippy -D warnings` red) and is never SLOP. Re-checked all three
SLOP items against it — **all three survive**; no reclassification needed. Rationale per item is
recorded inline in the SLOP rows below.

**Measured exception for this area — `lb-soak` does NOT carry `#![deny(missing_docs)]`.**
17 of 18 crate roots carry the attribute; `crates/lb-soak/src/lib.rs` deliberately scopes its deny
to the panic-freedom triad only (`clippy::unwrap_used, expect_used, panic`). Its own comment at
lines 23-29 states why: *"Scoped to the panic-freedom triad — the gate's actual intent — rather than
the full pedantic set the product crates carry, since lb-soak is a black-box test harness (no
missing_docs/indexing_slicing churn)."* Verified by parsing the attributes with comment lines
stripped (a naive `grep missing_docs` false-positives on that very comment).

Two further scope facts for the sweeper:
* `crates/lb-l4-xdp/ebpf/src/main.rs` carries no `missing_docs` either (`no_std` BPF crate).
* Every `tests/*.rs` is its own crate and none of them declare `missing_docs`, so the lint does not
  reach integration-test doc comments in this area.

In this area the only gate-protected items appearing anywhere in my lists are
`crates/lb-l4-xdp/src/lib.rs:3-5` (crate-level `//!` under the lint) and the `///` block enclosing
`loader.rs:1303` — both already classified KEEP/AMBIGUOUS, i.e. **correct in place, never delete**.

The lead calibration held exactly. Every `S<n>` / `F-S<n>-<m>` / `CF-…` / `ROUND8-L4-…` marker in
this area is a finding-ID attached to regression rationale, an RFC/kernel-UAPI citation, an
`unsafe` SAFETY note, a `#[allow]` justification, or a negative-control explanation. The three
SLOP items are all *stale factual claims about transient state* that have since been resolved —
not filler. The AMBIGUOUS list is the valuable output: 20 comments that are LOAD-BEARING in intent
but now **misstate current code**. They must be CORRECTED, not deleted; deleting them is an R3
knowledge regression, leaving them uncorrected keeps a false map of the binary in the tree.

## PROPOSED DELETIONS (SLOP)

`SLOP | crates/lb-l4-xdp/tests/elf_sections.rs:11-14 | "NOTE on the local sandbox: until `scripts/build-xdp.sh` is re-run against t" | FALSE today — verified `readelf -S src/lb_xdp.bin`: license + .BTF + .BTF.ext ALL present in the committed 35864-byte ELF. The note tells a future reader this test is expected-red. Delete ONLY through "…after which this test runs green."; the trailing sentence "The assertions are written strict-by-default so a stale ELF is caught…" is LOAD-BEARING and stays. GATE CHECK: safe twice over — (a) this is a PARTIAL-prose edit inside a `//!` block that is RETAINED, so the item keeps its doc and `missing_docs` cannot fire; (b) it is an integration-test crate, which declares no `missing_docs` at all.`
`SLOP | crates/lb-l4-xdp/tests/round8_conntrack_state.rs:16-17 | "NUM_SLOTS bumps with this commit from 13 to 15 to make room for `StatSlot::" | Pure changelog ("bumps with this commit") AND now wrong — NUM_SLOTS is 16 (L4-03 appended NewFlowRateCap). The correct, current invariant is already stated at the assertion site in the same file (lines 222-243), so removal loses zero information. GATE CHECK: same as above — partial-prose edit inside a RETAINED `//!` block, in a test crate with no `missing_docs`.`
`SLOP | crates/lb-soak/src/loadgen.rs:2839 | "// `u16::MAX` sentinel = \"do not respond\" (see h3_pair_build)." | Describes a sentinel that does not exist. The call is `h3_pair_build(None)` and `h3_pair_build`'s own doc (line 2578-2581) states the real contract ("`None` ⇒ the server drains the request but NEVER responds"). The comment is a superseded-design remnant that actively contradicts the adjacent line. GATE CHECK: unaffected — an ordinary `//` line INSIDE a private fn body in `mod tests` (explicitly in-scope per the lead's rule), and lb-soak carries no `missing_docs` at all.`

## AMBIGUOUS (KEEP + flag)

`AMB | crates/lb-soak/src/config_gen.rs:238-255 | "## F-S26-1 — this front is BACKEND-LESS in the production binary … The shipped `expressgateway` binary NEVER wires an HTTP backend onto a `protocol = \"quic\"` listener: `spawn_quic` → `quic_listener_params_from_config` does not call `with_backends`/`with_h3_backend`/`with_h2_backend`" | **HIGHEST-VALUE FLAG.** This premise is now FALSE: `crates/lb/src/main.rs:1736-1738` calls `wire_h3_terminate_backends`, which calls `with_backends` / `with_h2_backend` / `with_h3_backend`. F-S26-1 was FIXED. Proof in this same file: `quic_h3_terminate_h2` (line 317+) emits an h2 backend for sc9 and that scenario works. The *conclusion* (this sc7 config deliberately emits no backend, so THAT front is backendless) is still correct and load-bearing — only the "the binary cannot" rationale is stale. Needs rewrite, never deletion.`
`AMB | crates/lb-soak/src/bin/eg-soak.rs:592-599 | "F-S26-1: the production binary wires NO backend onto a `protocol=\"quic\"` listener, so this front is exercised on its INGRESS + the inline-400 decoded egress …" | Same stale premise as config_gen.rs:238. The sc7 scenario's own behaviour is unchanged (its config has no backend), but the stated reason is refuted by main.rs.`
`AMB | crates/lb-soak/src/loadgen.rs:1173-1184 | "F-S26-1: the production front is backend-less, so there are exactly two observable request outcomes, and BOTH are asserted (non-vacuous)" | Third copy of the same stale premise. The two-class client assertion is genuinely non-vacuous for the sc7 CONFIG and must be kept; the "production front is backend-less" framing is what needs correcting.`
`AMB | crates/lb/tests/quic_passthrough_e2e.rs:517-561 | "LB-mints-Retry handshake requires the backend know the client's ORIGINAL DCID … Both are passed via `quiche::accept_with_retry` + an explicit `quiche::RetryConnectionIds`" … "CF-S15-PASSTHROUGH-RETRY-ODCID closed by this commit" … "Until resolved, this real-quiche-handshake e2e is `#[ignore]`'d" | Internally self-contradictory AND contradicts the code three ways: (1) the test uses `mint_retry = false` (line 203) and plain `quiche::accept(&scid, None, …)` (line 257), NOT `accept_with_retry`/`extract_odcid_from_token_unsafe`; (2) it says the CF is "closed by this commit" and then says "until resolved"; (3) it claims the test is `#[ignore]`'d — it is NOT (line 562 has no ignore). CF-carry-forward text is sacred (standard §6), so KEEP, but this block needs an owner ruling on what is actually true.`
`AMB | crates/lb-l4-xdp/tests/xdp_link_id_drop_safe.rs:44-62 | "the `_link_id` returned by aya is dropped INSIDE the loader's `attach` method" | Stale: ROUND8-L4-12 changed `XdpLoader::attach` to RETAIN the link id in `self.attached_links` (loader.rs:1087-1088) precisely so `detach_verifying` can issue a real `Xdp::detach`. The compile-time signature tripwire above it is still valid (`attach` still returns `Result<(), _>`), so the test is not broken — only the scaffold prose is.`
`AMB | crates/lb-l4-xdp/tests/stats_export.rs:9 | "- The shape of the snapshot (10 slots; per-cpu inner vecs)." | Stale numeric: NUM_SLOTS is 16, and this file's own test at line 83 asserts 16. Correct the number rather than drop the line.`
`AMB | crates/lb-l4-xdp/tests/stats_export.rs:20 | "Cross-check against `crates/lb-l4-xdp/ebpf/src/main.rs:198-207` (the `STAT_*` constants)." | Stale line-range citation — the `STAT_*` constants now live at ebpf/src/main.rs:324-364. The cross-check itself is load-bearing (it reads the eBPF source at test time).`
`AMB | crates/lb-l4-xdp/tests/round8_backend_sentinel.rs:88-90 | "NUM_SLOTS is the floor invariant — ROUND8-L4-08 bumps it to 13 when fragment slots land." | Stale forward-looking changelog (NUM_SLOTS is 16). The `slots > 10` assertion it explains is still correct and deliberately loose.`
`AMB | crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs:28-34 | "Until that wiring lands, the assertion is gated by [`AUDIT_LINE_REQUIRED`] so this file COMPILES … Flip [`AUDIT_LINE_REQUIRED`] to `true` once builder-1 DMs that the audit line landed" | Completed-workflow narrative — the const IS already `true` (line 69). Reads as SLOP under §"session commentary", but it documents the still-live `else` branch's semantics, so the two readings compete. Keep unless the sweeper also removes the dead `else` arms.`
`AMB | crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs:58-68 | "Flip to `true` once builder-1's A3 audit-line wiring lands … FLIPPED true: builder-1's A3 wiring landed (integration tip `b8499ea2`)" | Same shape. The "FLIPPED true … tip b8499ea2" half is genuine provenance for a security-audit gate; the "flip to true once" half is spent instruction.`
`AMB | crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs:29-34 | "Until that wiring lands, the count assertions are gated by [`AUDIT_LINE_REQUIRED`] so this file COMPILES … Flip to `true` once builder-1 DMs the wiring landed." | Same as above (this file's const is also already `true`).`
`AMB | crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs:54-63 | "Flip to `true` once builder-1's A3 cap-hit-audit + throttle wiring lands … FLIPPED true: … (integration tip `b8499ea2`)" | Same as above.`
`AMB | crates/lb-l4-xdp/src/lib.rs:3-5 | "Provides userspace simulation of the L4 XDP data plane. Real eBPF programs cannot be tested in CI, so we simulate the conntrack table, Maglev consistent hashing, and hot-swap behavior." | Crate-doc is now materially incomplete/misleading: the crate also ships the REAL aya loader, bpffs check, netlink RTM_GETLINK query, and NIC blocklist — and F-ESC-1 does a real `BPF_PROG_LOAD` on this box. Needs widening, not deletion. GATE-PROTECTED: this is the crate-level `//!` under lb-l4-xdp's `#![deny(missing_docs)]` — deleting it turns `clippy -D warnings` red. Correct in place only.`
`AMB | crates/lb-soak/src/bin/eg-soak.rs:15-17 | "Scenarios: sc1_h1h1, sc1b_h1h2, sc2_h2h2, sc3_slowloris, sc4_modeb, sc5_modea, sc6_413teardown, sc7_h3terminate, sc8_ws_h1, sc8b_ws_h2." | Usage doc omits `sc8c_ws_h3` and `sc9_grpc_h3`, both present in `SCENARIOS` (lines 49-50) and both fully implemented. Operator-facing incompleteness.`
`AMB | crates/lb-l4-xdp/Cargo.toml:22-24 | "aya-obj's Object::parse is the kernel-free ELF inspector used by `XdpLoader::parse_object`" | Stale symbol: the method is `XdpLoader::program_names`; no `parse_object` exists. The dep rationale itself is load-bearing (it is why the real-ELF test runs without CAP_BPF).`
`AMB | crates/lb-l4-xdp/tests/l4_xdp_conntrack.rs:93-96 | "requires the EBPF-2-03 source change to be built into the ELF — until CI rebuilds, the map is still HASH and this test fails at the eviction-policy assertion" | Almost certainly stale: ebpf/src/main.rs declares `CONNTRACK: LruHashMap` and the committed ELF is current. Inside an `#[ignore]`'d scaffold, so it has never been contradicted by a run.`
`AMB | crates/lb/tests/panic_abort.rs:22-23 | "see the comment on `PANIC_TOTAL` in `lb/src/main.rs`" | Stale symbol reference — main.rs has `PANIC_TOTAL_COUNTER` + `PANIC_TOTAL_FALLBACK`, no `PANIC_TOTAL`. The deferred-to-REL-2-07/15 note itself is a legitimate rule-6 carry-forward.`
`AMB | crates/lb-l4-xdp/src/loader.rs:1303-1304 | "// TODO(L4-06): mirror this guard with `1..=128` when an IPv6 ACL trie ships (currently absent)." | A `//` line embedded INSIDE a `///` doc-comment block, splitting the rustdoc for `insert_acl_deny` in two. Content is load-bearing (rule 6 deferred-because). Flagged as a formatting defect, not a deletion candidate — converting it to `///` or moving it above the doc block is the behaviour-neutral fix. NOTE: the ENCLOSING `///` block is on a `pub fn` under lb-l4-xdp's `#![deny(missing_docs)]` and is therefore gate-protected; only the embedded `//` line is in play, and it must be relocated rather than dropped.`
`AMB | crates/lb-l4-xdp/src/loader.rs:1599 | "return true; // disabled" | Leans SLOP: the doc comment 25 lines above already states "A `refill_per_sec` of `0` disables the gate (every `try_admit` returns `true`)". Retained as AMBIGUOUS because the one-word label does frame the branch. Zero-risk either way; not worth an edit on its own.`
`AMB | crates/lb-l4-xdp/tests/round8_verify_xdp_gate.rs:67-69 | "Once real logs are committed (post-CI), this assertion is expected to start failing — at which point the test should be updated to assert structural verifier-log shape instead." | A deliberate self-documenting time-bomb on the 5.15/6.1/6.6 placeholder baselines. F-ESC-1 has already committed a REAL 7.0 baseline (different kver, so this has not fired yet). Load-bearing as written; flagged so the lead knows the trigger condition is now half-met.`

## LOAD-BEARING NOTABLES (explicitly preserved)

`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:496-510 | ROUND8-L4-09: why every `ptr_at`/`ptr_at_mut` addition uses `checked_add` — aya#1562 scalar/pointer reordering + CVE-2022-23222 bounds-check-elision class, and the note that the verifier-log baselines must be refreshed after any BPF source change. Deleting this invites a "simplify" back to raw `+`.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:120-121, 179, 272, 316, 444-445 (loader.rs) and every `// SAFETY:` in ebpf/src/main.rs (467, 480-481, 524, 538, 546, 623, 656, 841, 853, 876, 932, 947, 1074, 1082, 1092, 1113) | MANDATORY unsafe-safety comments over packed-field reads/writes, per-CPU map pointers, and Pod layout assertions. Non-negotiable, never shortened.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:152-159 | ROUND8-L4-02: why RST/FIN-ACK prune exists at all — the sliding-RST replay attack that pure LRU is vulnerable to (Cilium bpf/lib/conntrack.h). This is the entire justification for the prune branches.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:398-419 | ROUND8-L4-03: the Katran `is_under_flood()` lesson-4 rationale + the explicit "this commit changes BPF source ⇒ verifier baselines must be refreshed" gate note.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:237-264 | ROUND8-L4-04 scope note: why `BACKENDS_V4` exists but is only touched behaviorally-inertly, and why the hot-path read is deferred to Pillar 4b-3 (verifier-log re-capture cost). Removing it makes `backend_table_published` look like dead code.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:940-941 | "Verifier will not accept an unbounded loop; a fixed small count is fine." — the kernel-verifier constraint behind the hard 2-extension-header cap.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:82-87 | Why the `const _: () = { let _ = …; }` anchor block exists (keeps header-size constants alive through refactors). Without it the block reads as deletable dead code.`
`KEEP | crates/lb-l4-xdp/ebpf/src/main.rs:667-671, 965-968 | ROUND8-L4-08 fragment guards with RFC 791 §3.1 / RFC 2460 §4.5 citations and the "no in-XDP reassembly (Katran/Cilium design)" decision.`
`KEEP | crates/lb-l4-xdp/src/loader.rs:110-117, 130-133, 170-172, 308-309 | CODE-2-07 padding-zero-init contract: exactly why `pub pad` stays public and why callers must funnel through `::new`. Pairs with tests/pod_padding.rs.`
`KEEP | crates/lb-l4-xdp/src/loader.rs:213-226, 344-351, 609-621 | ROUND8-L4-01 Katran-lesson-10 zero-IP/zero-port sentinel: the silent-drop vector and why `try_new` is the admission gate with an eBPF runtime mirror.`
`KEEP | crates/lb-l4-xdp/src/loader.rs:379-408 | Byte-size layout assertions with the per-field arithmetic. This is what keeps the userspace mirror byte-identical to the BPF map value; aya rejects a drifted accessor.`
`KEEP | crates/lb-l4-xdp/src/loader.rs:522-535 | EBPF-2-04 `is_unsupported_mode`: why ONLY EOPNOTSUPP(95)/EINVAL(22) trigger ladder fall-through, coded as literals to avoid a libc dep. Widening this set would silently swallow real bugs.`
`KEEP | crates/lb-l4-xdp/src/loader.rs:1092-1101 | Why `Native`/`Hw` intentionally SKIP the ladder — "a loud startup failure rather than a silent 10-50x throughput regression to SKB".`
`KEEP | crates/lb-l4-xdp/src/loader.rs:1416-1432, 1480-1518 | ROUND8-L4-12: the EBUSY-on-redeploy close — why the pre-check is real now (not the old `prog_id: None` stub) and why detach-then-attach replaces a single-syscall BPF_F_REPLACE given the aya 0.13.1 floor.`
`KEEP | crates/lb-l4-xdp/src/nic_compat.rs:115-128 + 144-159 + 239-253 | F-COR-7: the ena driver+kernel fallback key. Encodes exactly why an UNRESOLVED firmware must not fail-open on a kernel-keyed row, and why the 6.7 boundary comes from the row's own documented condition rather than a fleet-wide guess. Deleting this re-opens a dead defence path OR causes a fleet-wide native-XDP regression.`
`KEEP | crates/lb-l4-xdp/src/nic_compat.rs:403-420 | The aya-0.13.1 `BPF_PROG_TEST_RUN` API-blocker note that justifies `probe_xdp_silent_drop()` returning `ProbeUnavailable`. Paired with the real Cargo.lock tripwire test.`
`KEEP | crates/lb-l4-xdp/src/netlink_xdp.rs:20-40 | The kernel netlink wire-format diagram (RTM_GETLINK / IFLA_XDP nesting) plus the panic-freedom guarantee ("no slice indexing — every read goes through `.get()`") that the byte-parser proof depends on.`
`KEEP | crates/lb-l4-xdp/src/netlink_xdp.rs:148-152 | "The kernel never hands out prog_id 0; treat 0 as none so attach_replacing does not think a foreign prog 0 owns the iface." — a WHY-note preventing a real misattribution bug.`
`KEEP | crates/lb-l4-xdp/src/stats_export.rs:185-188, 265-278 | The wire-stable slot-ordering contract and the ROUND8-L4-05 note on why `AttachProbeFailed` (16) is deliberately NOT counted in `NUM_SLOTS`. Getting this wrong corrupts every operator's `xdp_packets_total{result}` labels or unbounds the kernel read loop.`
`KEEP | crates/lb-l4-xdp/tests/xdp_attach_mode.rs:115-136 | The two ENA hard preconditions for a genuine DRV attach (MTU ≤ 3498, combined channels ≤ half max) with the exact kernel error strings, plus why lowering them is safe for the control SSH session. Hard-won operational knowledge that cannot be re-derived from code.`
`KEEP | crates/lb-l4-xdp/tests/xdp_attach_mode.rs:441-452 | Why native mode is proven by the presence of ` xdp `/`prog/xdp` and the ABSENCE of `xdpgeneric`/`xdpoffload` under iproute2 6.19 (the legacy `xdpdrv` token is no longer emitted). This is the non-vacuity argument for the D-1 gate.`
`KEEP | crates/lb-soak/src/bin/eg-soak.rs:1023-1039 (`ws_gauges`) | The F-S20-2 leak DISCRIMINANT: why `fds` is the connection-leak signal and why `accept_inflight` is deliberately excluded (low-baseline sawtooth under churn makes the relative-growth analyzer read a 0→2 wiggle as "+200%"). Sacred negative-control reasoning; deleting it invites re-adding a false-positive gauge.`
`KEEP | crates/lb-soak/src/loadgen.rs:1039-1048 | F-S20-1: why the QUIC client must LOOP `stream_send` on a partial write (cwnd shared across streams), and that sending once is what made S20 misread a load-client truncation as a gateway relay stall. The canonical "symptom ≠ attribution" note in this area, with its own regression test at line 2466.`
`KEEP | crates/lb-soak/src/loadgen.rs:368-376 + 405-411 | F-S27-2: why the WS-over-H2 load client READS NORMALLY and releases flow-control capacity — it deliberately does NOT exercise the gated unbounded-buffer DoS. Removing this makes a future "simplification" to a non-reading client look harmless while silently changing what the soak proves.`
`KEEP | crates/lb-soak/src/loadgen.rs:1960-1963 | Why WS-over-H3 frames use BINARY not Text (RFC 6455 §5.6 UTF-8 validity — tungstenite correctly tears the tunnel down on non-UTF-8 Text). A one-line change here would produce a mystifying flake.`
`KEEP | crates/lb-soak/src/loadgen.rs:1546-1555 | The S21 lesson encoded as a rule: a load client must HONOR `StreamBlocked` flow-control rather than mis-count it as a gateway failure; plus why the `loop { break id }` shape exists (panic-freedom deny lint, S34).`
`KEEP | crates/lb-soak/src/timeseries.rs:349-371 + tests 479-494 | The BOUNDED/DRIFT thresholds with their derivations (1.8x slope ratio separating linear from quadratic; the rel-growth gate that protects a high-monotone sawtooth from a false leak verdict). This is the soak verdict's entire calibration.`
`KEEP | crates/lb-soak/src/chaos.rs:209-230 | CF-S19 (S21) sharper teardown-vs-error-head race: why a cheap oversize-HEADER 4xx exercises the same flush-vs-teardown window as a 413, and why flooding 64 MiB bodies is the S20 anti-pattern. Explains a non-obvious fixture choice.`
`KEEP | crates/lb/src/main.rs:597-610 (`rebuild_l7_proxies`) | The S37-C HONESTY INVARIANT: the exact list of fields this rebuild applies, what it deliberately preserves (the shared `HooksBundle`), and that changing the set here without reclassifying it in `LbConfig::diff` breaks the invariant the verifier adversarially tests.`
`KEEP | crates/lb/src/main.rs:1727-1738 + 1774-1788 | F-S26-1 wiring: the H3-terminate → backend dispatch table (h1/tcp → with_backends, h2 → with_h2_backend, h3 → with_h3_backend) and the "no backends ⇒ byte-identical to before (R3)" guarantee. Directly contradicts the three stale soak comments flagged above — this is the CORRECT source of truth.`
`KEEP | crates/lb/src/main.rs:2982-2986 + 3292-3304 | SIGNAL-LOSS FIX (S37-C, R6): why the lifecycle-signal streams are installed ONCE outside the loop — re-installing per iteration lost a SIGTERM landing while a SIGHUP was being serviced. Canonical "use X not Y because Y does Z".`
`KEEP | crates/lb/src/main.rs:3553-3560 + 3605-3610 | OPS-04+L4-12 cases C-2/C-3/C-15: the biased cancel arm AND the synchronous post-accept tail-check, with the exact consequence of dropping the latter (per-IP counter drift + leaked accepted fd). Deleting this re-opens a measured bug class.`
`KEEP | crates/lb/src/main.rs:3917-3932 | ROUND8 OPS-02 div-l7 per-connection drain jitter: why intra-pod spread is drawn per-connection on top of the coordinator's per-process draw, and that `0` collapses to the original immediate-abort behaviour.`
`KEEP | crates/lb/src/main.rs:2793-2798 | F-RES-5 (S38): the Watchdog sweeper is OBSERVABILITY-ONLY — it must NOT close the socket (that would race the drain coordinator); enforcement belongs to the timeout stack. A "fix" that made it close sockets would be a regression.`
`KEEP | crates/lb/src/main.rs:1563-1567 + 1712-1725 | CF-S27-2 / WS gating: `h2_extended_connect` and `h3_extended_connect` are OFF by default and the settings frame stays byte-identical (R3) unless opted in. This is a security gate's rationale.`
`KEEP | crates/lb/src/main.rs:5738-5747 + 5717-5725 | R8 wired-tunnel backpressure: what the plateau proves, that it is VOLUME-INDEPENDENT, and why the CLIENT's quiche windows must be capped (auto-tune to 16/24 MiB would let the test client absorb the flood and mask the gateway's backpressure). Non-vacuity argument for the whole test.`
`KEEP | crates/lb/src/main.rs:5554-5560 + 6067-6074 | R13 reset-vs-EOF negative control and the bare-FIN-is-abnormal (RFC 6455 §7.1.5) proof — "the gateway must NOT fabricate a clean Close from a bare FIN".`
`KEEP | crates/lb/src/xdp.rs:60-74 + 100-122 | SEC-2-11 CAP_BPF→CAP_SYS_ADMIN fallback policy: why "no CAP_BPF" is treated as a pre-5.8-kernel signal rather than a failure, and why a probe error is swallowed for CAP_BPF ONLY. Removing it invites "simplifying" the fallback away.`
`KEEP | crates/lb/src/lib.rs:13-25 | Why both main.rs and lib.rs declare `mod xdp;` (separate crates, same file, no runtime duplication) plus the rule that binary-only wiring MUST stay private. Pre-empts a wrong "de-duplication".`
`KEEP | crates/lb/build.rs:1-5 + crates/lb-l4-xdp/build.rs:1-12 | Why the `cfg(lb_xdp_elf)` check is duplicated per consumer (cargo cfg values do not propagate across crates) and the EBPF-2-01 64 KiB ELF budget with its sync obligation to tests/elf_sections.rs.`
`KEEP | crates/lb-l4-xdp/src/bpffs.rs:25-29 + 56-58 + 68-77 | BPF_FS_MAGIC redeclared next to its use site (libc ships no constant), the statfs SAFETY note, and the `#[allow(..., reason = "buf.f_type type varies across libc versions / arches")]` justification.`
`KEEP | crates/lb-soak/Cargo.toml:63-70 + crates/lb/Cargo.toml:21-27 | Dependency-edge provenance: the `h2 = "0.4"` owner ruling with the "verified `git diff Cargo.lock` shows only new edges, no version bumps" evidence, and the arc-swap re-add citing CODE-2-12's explicit anticipation. These are exactly what a future editor needs before pruning a dep.`

## Per-file load-bearing counts

crates/lb/src/main.rs : 392
crates/lb/src/xdp.rs : 23
crates/lb/src/lib.rs : 1
crates/lb/build.rs : 1
crates/lb/Cargo.toml : 13
crates/lb/tests/informational_pass_through_main.rs : 4
crates/lb/tests/panic_abort.rs : 11
crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs : 17
crates/lb/tests/quic_passthrough_bounded_state.rs : 26
crates/lb/tests/quic_passthrough_cid_migration.rs : 6
crates/lb/tests/quic_passthrough_e2e.rs : 34
crates/lb/tests/quic_passthrough_metrics.rs : 18
crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs : 18
crates/lb/tests/quic_passthrough_strict_source_binding.rs : 11
crates/lb/tests/xdp_cap_probe.rs : 9
crates/lb-l4-xdp/Cargo.toml : 4
crates/lb-l4-xdp/build.rs : 4
crates/lb-l4-xdp/ebpf/Cargo.toml : 4
crates/lb-l4-xdp/ebpf/rust-toolchain.toml : 1
crates/lb-l4-xdp/ebpf/src/main.rs : 116
crates/lb-l4-xdp/src/bpffs.rs : 13
crates/lb-l4-xdp/src/lib.rs : 51
crates/lb-l4-xdp/src/loader.rs : 214
crates/lb-l4-xdp/src/netlink_xdp.rs : 36
crates/lb-l4-xdp/src/nic_compat.rs : 53
crates/lb-l4-xdp/src/sim.rs : 57
crates/lb-l4-xdp/src/stats_export.rs : 59
crates/lb-l4-xdp/tests/elf_sections.rs : 2
crates/lb-l4-xdp/tests/l4_xdp_conntrack.rs : 7
crates/lb-l4-xdp/tests/loader_license_assert.rs : 11
crates/lb-l4-xdp/tests/pod_padding.rs : 30
crates/lb-l4-xdp/tests/real_elf.rs : 2
crates/lb-l4-xdp/tests/round8_acl_admission.rs : 8
crates/lb-l4-xdp/tests/round8_atomic_backends.rs : 12
crates/lb-l4-xdp/tests/round8_attach_probe.rs : 21
crates/lb-l4-xdp/tests/round8_attach_replace.rs : 9
crates/lb-l4-xdp/tests/round8_backend_flags.rs : 4
crates/lb-l4-xdp/tests/round8_backend_sentinel.rs : 5
crates/lb-l4-xdp/tests/round8_bpffs_check.rs : 7
crates/lb-l4-xdp/tests/round8_conntrack_state.rs : 14
crates/lb-l4-xdp/tests/round8_ena_kernel_blocklist.rs : 2
crates/lb-l4-xdp/tests/round8_fragments.rs : 11
crates/lb-l4-xdp/tests/round8_netlink_xdp_query.rs : 30
crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs : 12
crates/lb-l4-xdp/tests/round8_synflood_cap.rs : 11
crates/lb-l4-xdp/tests/round8_verifier_baseline_70.rs : 6
crates/lb-l4-xdp/tests/round8_verify_xdp_gate.rs : 7
crates/lb-l4-xdp/tests/stats_export.rs : 5
crates/lb-l4-xdp/tests/xdp_attach_mode.rs : 35
crates/lb-l4-xdp/tests/xdp_link_id_drop_safe.rs : 3
crates/lb-l4-xdp/tests/xdp_pin_paths.rs : 5
crates/lb-soak/Cargo.toml : 7
crates/lb-soak/src/backends.rs : 35
crates/lb-soak/src/bench.rs : 28
crates/lb-soak/src/bin/eg-bench.rs : 14
crates/lb-soak/src/bin/eg-soak.rs : 43
crates/lb-soak/src/chaos.rs : 25
crates/lb-soak/src/config_gen.rs : 17
crates/lb-soak/src/gateway.rs : 17
crates/lb-soak/src/lib.rs : 3
crates/lb-soak/src/loadgen.rs : 147
crates/lb-soak/src/metrics.rs : 16
crates/lb-soak/src/procstat.rs : 14
crates/lb-soak/src/sampler.rs : 9
crates/lb-soak/src/timeseries.rs : 50
