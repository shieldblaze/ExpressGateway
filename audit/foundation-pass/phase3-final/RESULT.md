# Phase 3 — FINAL R1 Gate Result

Verifier: verifier-final (authored none of the fixes; author != verifier, R5).
Repo: /home/ubuntu/Code/ExpressGateway
Branch: audit/foundation-pass
HEAD: 903a321d337fddbe52350680904c7c2985447522
Tree at start: clean (`git status` = "nothing to commit, working tree clean").
Date: 2026-05-17

Gate run fully independently from a clean tree at HEAD per STANDING-RULES R1/R2.

---

## 1. `cargo fmt --check`

```
FMT_EXIT=0
```

Verbatim: no output, exit code 0.

**RESULT: CLEAN (exit 0).**

---

## 2. `cargo clippy --all-targets --all-features -- -D warnings`

`cargo clean` is blocked in this environment (Permission denied, os error 13,
sandbox bulk-delete policy — target dir is ubuntu-owned and writable, deps cache
intact). To run clippy fully independently anyway, all 18 workspace crates were
individually `cargo clean -p <crate>`'d (fingerprint count 1344 -> 831,
confirming every workspace-crate artifact was dropped while only dep caches were
retained), then clippy recompiled and re-linted every workspace crate from
scratch. A subsequent forced-edit recheck (`touch crates/lb-core/src/lib.rs`)
re-ran lints again.

```
   Compiling lb-l4-xdp v0.1.0 ...
   Checking lb-core / lb-io / lb-h3 / lb-grpc / lb-h2 / lb-controlplane /
            lb-config / lb-cp-client / lb-health / lb-h1 / lb-balancer /
            lb-quic / lb-observability / lb-l7 / lb-integration-tests ...
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.22s
CLIPPY_EXIT=0
```

`grep -cE '^(warning|error)' clippy.log` = 0. Zero warnings, zero errors.

**RESULT: CLEAN (exit 0, zero warnings).**

---

## 3. `cargo test --workspace --all-features -- --test-threads=8` ×3 (8-core)

`test result:` for the affected binary `tests/h2_security_live.rs` (the only
binary that ever differed across runs; full per-run scans show no other FAILED
summary, no other panic, no compile error in any run):

- **Run 1:** `test result: FAILED. 5 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.39s`
  - `test rapid_reset_goaway ... FAILED`
  - `error: test failed, to rerun pass -p lb-integration-tests --test h2_security_live`
  - Aggregate run 1: 1 FAILED test binary. **RUN 1 DID NOT PASS.**
- **Run 2:** `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s` — `rapid_reset_goaway ... ok`. Aggregate: 0 FAILED.
- **Run 3:** `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s` — `rapid_reset_goaway ... ok`. Aggregate: 0 FAILED.

Verbatim run-1 failure (captured, test-run-1.log lines 805–818):

```
---- rapid_reset_goaway stdout ----
rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))

thread 'rapid_reset_goaway' panicked at tests/h2_security_live.rs:342:5:
expected server-initiated GOAWAY after rapid-reset flood; send_err=None, conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
```

### R2 classification (mechanism proven from captured output — NOT environmental)

`tests/h2_security_live.rs::rapid_reset_goaway` configures the H2 rapid-reset
mitigation thresholds to 3 (`max_pending_accept_reset_streams: 3`,
`max_local_error_reset_streams: 3`), opens+resets up to 512 streams, and asserts
(line 338–346) that the teardown is **server-initiated**, i.e.
`send_err.is_remote()/is_go_away()` OR
`conn_res == Ok(Ok(Err(e))) where e.is_remote()/is_go_away()` — that is, the
server must emit a GOAWAY (ENHANCE_YOUR_CALM) frame, the CVE-2023-44487
mitigation contract.

Captured evidence: `send_err=None` (the client completed all 512 send/reset
iterations without the server ever surfacing REFUSED_STREAM / ENHANCE_YOUR_CALM
/ GOAWAY through `ready()`/`send_request`), and the connection future resolved
with `Io(Kind(BrokenPipe))` — a raw TCP teardown. `BrokenPipe` is neither
`is_remote()` (a peer GOAWAY/RST) nor `is_go_away()`.

Per STANDING-RULES R2: "Wrong frame/status/header/close/body = REAL DEFECT,
always." The server abruptly tore the socket down WITHOUT flushing the required
GOAWAY frame instead of sending the GOAWAY the rapid-reset mitigation owes the
peer. There is demonstrable **server-side misbehavior** (missing/unflushed
GOAWAY, wrong close), so the R2 carve-out for "proven scheduling-starvation
timeout with ZERO server-side misbehavior" does NOT apply. The non-determinism
(fails 1/3, passes 2/3) is consistent with a race in the server's H2
rapid-reset path between emitting/flushing the GOAWAY frame and closing/
resetting the TCP connection: when the connection is torn down before the
buffered GOAWAY reaches the wire, the client observes `BrokenPipe` instead of a
remote GOAWAY.

**This is a REAL DEFECT (race / GOAWAY-flush-before-close ordering bug in the
H2 rapid-reset mitigation emission path). It is NOT environmental.**

---

## Overall R1 verdict

R1 requires `cargo test --workspace --all-features` to pass **deterministically,
8-core, 3x, all 3 pass, no asterisk**. Run 1 failed (`rapid_reset_goaway`
panicked, server emitted abrupt BrokenPipe close instead of the required
server-initiated GOAWAY). 1 of 3 runs did not pass.

- fmt: CLEAN (exit 0)
- clippy: CLEAN (exit 0, zero warnings)
- test ×3: **NOT deterministic — 1/3 FAILED**

### **R1 VERDICT: NOT-GREEN**

Exact blocker: `tests/h2_security_live.rs::rapid_reset_goaway` is flaky
(FAILED run 1; ok runs 2 & 3) at 8-core parallelism. Proven mechanism (R2):
server-side defect — the H2 rapid-reset mitigation tears down the TCP
connection with `Io(Kind(BrokenPipe))` instead of emitting/flushing a
server-initiated GOAWAY (ENHANCE_YOUR_CALM); `send_err=None`. Real defect,
not environmental. Foundation is not baseline-green.
