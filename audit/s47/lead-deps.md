# S47 — lead findings: dependency security

Author: lead (not a delegated agent). Verified independently.

---

## LEAD-DEP-1 — RUSTSEC-2026-0258: vulnerable `h2` in the production H2 data path

**Severity: HIGH for this deployment** (advisory itself rates Low — see below).
**Status: FIXED** in commit `29199144` (h2 0.4.14 -> 0.4.19).

### Evidence

The weekly `Dependency Audit (weekly, strict)` job has been RED since
2026-08-17. Run `33318546116` (2026-08-30):

    error: 1 vulnerability found!
    error: 1 denied warning found!
    Crate:    h2
    Version:  0.4.14
    Title:    h2 unbounded empty DATA frames
    ID:       RUSTSEC-2026-0258
    Solution: Upgrade to >=0.4.16

### It is a production path, not dev-only

`Cargo.toml` lists `h2 = "0.4"` under `[dev-dependencies]` (the malicious-client
harness in `tests/h2_security_live.rs`), which makes it *look* dev-only. It is
not. From `Cargo.lock`, the dependents of `h2` are:

    hyper, lb-integration-tests, lb-soak, reqwest

`hyper` is a production dependency of `lb-l7`, and `lb-l7::h2_proxy` is the
live HTTP/2 front. The vulnerable codec is in the shipped binary.

### Why HIGH here rather than Low

The advisory says the flaw causes "unbounded memory usage, **or a panic if the
length overflows**". Our release profile is `panic = "abort"`
(workspace `Cargo.toml`, `[profile.release]`), chosen deliberately so a panic
cannot unwind through the 17 `unsafe` blocks in `lb-io::ring`. The consequence
is that h2's panic is not a failed stream or a failed connection — it aborts
the whole gateway process, dropping every connection it is currently serving.
A per-connection memory bug in a library becomes a whole-node availability loss
in this deployment.

### The bump fixes more than the advisory

The advisory names one issue. `0.4.14 -> 0.4.19` also crosses these, from the
h2 CHANGELOG — several are remote-input panics, i.e. process aborts here:

  0.4.15  Fix overflow calculating padding length if a DATA frame had 255
          bytes of padding.                         <- attacker-controlled panic
  0.4.15  Fix decoding panic with an absurd amount of headers and no limit
          to now use try_append().                  <- attacker-controlled panic
  0.4.15  Fix rejecting frames on streams whose HEADERS have not been sent.
  0.4.15  Fix discarding of buffered DATA frames when a reset is scheduled.
  0.4.16  Fix limiting excessive amount of small DATA frames.   <- the advisory
  0.4.16  Fix releasing of flow control capacity earlier, when RecvStream is
          dropped.                                  <- flow-control capacity leak
  0.4.16  Fix busy-looping when IO write returns 0 to mean connection closed.
                                                    <- CPU spin / DoS
  0.4.16  Fix resets received after END_OF_STREAM to allow the data to still
          be received.
  0.4.17  Fix limiting of excessive small DATA frames to ignore EOS frames.
  0.4.17  Fix HPACK encoding table to cap the max size to 4kb.

0.4.15 shipped 2026-06-15 and the lockfile held 0.4.14 (2026-05-04), so the two
remote-panic fixes had been available for two and a half months.

### Behavioural change to watch

0.4.18 adds `data_frame_budget(n)` to the client/server builders and 0.4.19
tunes the default automatic budget from the configured connection window. This
is the security fix's mechanism: h2 now bounds excessive *small* DATA frames by
default. Our streaming tests push large frames, not small ones, so the budget
is not expected to bite — but this is the one behavioural risk in the bump and
it is why the change is CI-verified rather than assumed.

### Fix applied

Surgical `Cargo.lock` edit: 2 lines (version + checksum). No resolver churn, no
other crate moved. `cargo update -p h2 --precise` was tried first and rejected
because it also reshuffled 8 `windows-sys` selections — irrelevant on a
Linux-only project and pure review noise. The dependency set of h2 is
byte-identical between 0.4.14 and 0.4.19 (verified against the crates.io
dependencies API), so the hand-edit is exact.

`tokio` remains at 1.51.1. The `<1.52` hold is untouched.

---

## LEAD-DEP-2 — yanked `chacha20 0.10.0`

**Severity: LOW.** **Status: FIXED** in commit `29199144` (0.10.0 -> 0.10.2).

Reached via `rand 0.10`, a production workspace dependency. Both 0.10.0 and
0.10.1 are yanked upstream; 0.10.2 (2026-08-27) is the first unyanked release.
No advisory attached — the risk is supply-chain hygiene: the lockfile pinned a
version withdrawn from the registry. Same dependency structure (`cfg-if`,
`cpufeatures`, optional `rand_core ^0.10`), so the hand-edit is exact.

---

## LEAD-DEP-3 — the update pipeline could not deliver a security fix

**Severity: MEDIUM (process).** **Status: FIXED** in commit `4d78ea4a`.

This is the root cause of LEAD-DEP-1 sitting unfixed for two weeks, and it is
the more durable finding of the two.

`.github/dependabot.yml` put every cargo update into a single group
(`patterns: ["*"]`) behind `open-pull-requests-limit: 1`. That group always
carried the `tokio` bump this repo deliberately holds
(CF-S37-D-TOKIO-1.52-RELAY: tokio 1.52 collapses H2->H3 relay throughput ~10x).
So the PR was closed on sight and regenerated the next day, and **every security
fix travelling inside it was discarded along with it.**

Confirmed directly from the Dependabot job log (run `33366052679`):

  - `"ignore-conditions":[]` — no holds were ever expressed to Dependabot, so it
    rewrote the manifest constraint daily.
  - Grouped PR **#259** contains `tokio 1.53.1` and `h2 0.4.15` in the same
    bundle, plus 21 other bumps.
  - Note `h2 0.4.15` — the bundle would **not** have cleared the advisory even if
    it had been merged, because the patched version is >= 0.4.16. A
    *version*-update group proposes the version that was current when the group
    was formed; only a *security* update targets the patched floor.

### Fix applied

Split the cargo group by `applies-to`, so security updates open their own PR and
can land without being rejected alongside routine bumps; raised the PR limit
1 -> 2 so both can be open at once (without this the split has no effect); and
added narrow `ignore:` entries for the two genuinely-held deps:

    tokio   >= 1.52   (CF-S37-D-TOKIO-1.52-RELAY)
    reqwest >= 0.13   (CF-S37-D-REQWEST-0.13)

Both bounds block only the held major/minor and leave patch releases —
including security patches — free to flow.

---

## LEAD-DEP-4 — the fuzz crate links a different quiche than production

**Severity: LOW.** **Status: reported, not yet fixed.**

`fuzz/Cargo.toml:34` pins `quiche = "0.28"` while the workspace moved to
`quiche 0.29` at S31 (`Cargo.toml:113`). The fuzz crate is deliberately outside
the workspace (it needs nightly + libFuzzer), so nothing forces the two into
agreement and no gate compares them.

Consequence: the QUIC/H3 fuzz targets exercise a codec that is one minor version
behind the one actually shipped. Whatever they prove, they prove about 0.28.
Coverage of the real 0.29 paths is weaker than the presence of a green
`fuzz-smoke` CI job suggests. Not exploitable on its own; it degrades an
assurance mechanism rather than the product.

Deferred to triage rather than fixed inline: bumping it changes what the fuzz
targets link and belongs with the fuzz-target review, not with a lockfile patch.
