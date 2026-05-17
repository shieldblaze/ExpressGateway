# builder-3 — F-COR-9 (task #17) plan

## Finding
`tests/reload_zero_drop.rs::test_sigterm_drains_h2_with_goaway` fails ~1/19
under full `cargo test --workspace --all-features -- --test-threads=8`
8-core contention with rustls `InvalidMessage(InvalidContentType)` at
reload_zero_drop.rs:1045 — the TLS handshake stage, BEFORE any
GOAWAY/drain code. Isolated 5/5 pass.

## Proven mechanism (from source + signature; R2 = REAL DEFECT)

Two compounding harness defects, both a port/global-state collision:

(A) `ephemeral_port()` (l.188-193): binds `127.0.0.1:0`, reads the
kernel-assigned port P, then `drop(l)` — RELEASING P. Port P is now
unowned. backend + listener each call it; dozens of concurrent
test processes under `--workspace --all-features --test-threads=8`
race the same kernel ephemeral pool. Classic TOCTOU.

(B) `spawn_gateway()` (l.324-349) readiness gate is a bare
`TcpStream::connect_timeout(&addr)` that returns Ok the instant ANY
process completes a TCP handshake on `addr`, then drops the probe
socket. It proves "something accepts TCP on P", NOT "the gateway's
TLS accept path is live on P".

Failure window: under 8-core contention the gateway cold-start (spawn
+ config parse + self-signed TLS load + bind) is slow. During it a
DIFFERENT concurrent process can grab the released port P; the bare
TCP probe connects to that FOREIGN socket and returns "ready"; the
test then does the real `TcpStream::connect` + rustls
`connector.connect()` (l.1041-1045) against a non-TLS / foreign peer
→ first inbound byte is not a valid TLS record ContentType →
`InvalidMessage(InvalidContentType)` at the rustls RECORD layer,
before ALPN. Exactly "connected to something that is not the expected
TLS server stream" = the foreign-port-reuse / stale-fd signature, not
a drain bug — matches builder-1's characterization (handshake fails
at :1045 before any GOAWAY code; ~1/19 under load; 5/5 isolated).

## Fix (root, deterministic; test-harness-only — gateway takes only an
## address from config, no fd-passing, so the fix is in the harness)

Make the readiness gate TLS-FAITHFUL: `spawn_gateway` for the H2/h1s
test must not return until a probe that completes the EXACT
TLS+ALPN(`h2`) handshake the test relies on succeeds against `addr`.
Only the real gateway (its self-signed cert + h1s listener) can
satisfy that handshake; a foreign reused-port socket or a stale fd
cannot. This closes BOTH the foreign-port-reuse window and the
stale-fd window at the root: the test connects only after the
*gateway itself* owns P and is serving TLS. If a foreign process
holds P for the whole boot budget the probe never succeeds and
spawn_gateway panics with a clear, correctly-attributed diagnostic
(deterministic failure, not a corrupt-handshake flake) — in practice
the transient holder releases well within boot_timeout and the
polling loop succeeds. Implemented as a new
`spawn_gateway_tls_ready(bin, cfg, addr, alpn)` used by the H2 test;
the existing plain TCP `spawn_gateway` (H1 plaintext path) is
unchanged. No assertion weakened, no test skipped/ignored/deleted.

## Proof method
1. Pre-fix: reproduce `InvalidMessage(InvalidContentType)` at
   reload_zero_drop.rs:1045 by looping the reload_zero_drop binary
   under a concurrent full `cargo test --workspace --all-features --
   --test-threads=8` 8-core load. Capture verbatim.
2. Post-fix: same contended condition, the whole reload_zero_drop
   binary (incl. test_sigterm_drains_h2_with_goaway) GREEN for >=30
   consecutive runs, zero handshake failures. Capture verbatim.
3. Commit (no push). Author != verifier.
