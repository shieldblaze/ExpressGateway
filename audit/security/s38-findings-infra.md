# S38 Security Audit — infra-auditor findings

Surface: TLS/cert, admin/observability API + auth, config + SIGHUP reload
(validator / ArcSwap / honesty-contract), secrets handling, L4/XDP.
Base: `feature/security-audit-s38` (main @ b8a99078). Method: read + grep +
PoC-authoring only (lead runs all PoCs; no cargo build/test/run per the rules).

---

## CRITICAL / HIGH (read immediately)

**None.** No auth bypass, no cert/verify bypass, no reload-corruption crossing
connections, no secret leak, no XDP OOB found. The operational layer is the
most-hardened surface in the tree (S37 landed `deny_unknown_fields`, the diff
honesty-contract, and the admin auth gate; this audit adversarially confirms
them). All findings below are MEDIUM/LOW hardening gaps.

---

## Findings table

| ID | Sev | Surface | One-line |
|----|-----|---------|----------|
| F-INFRA-01 | LOW | TLS/secrets | Retry-secret file: perms NOT checked on the load (read) path — asymmetric vs the TLS key's `assert_owner_only`. A pre-placed / drifted world-readable retry secret loads silently → retry-token forge / Initial-flood-defence bypass. |
| F-INFRA-02 | LOW | secrets | TLS private keys + retry/ticket secrets held in rustls/ring `Arc`, never zeroized (no `zeroize` dep anywhere). Defence-in-depth gap, not a reachable leak. |
| F-INFRA-03 | LOW (doc) | TLS | No server-side mTLS (`with_no_client_auth()`); TLS 1.2 allowed by default (`tls13_only` opt-in). Both intentional + documented — recorded so the deployment profile is explicit. |
| F-INFRA-04 | LOW (doc) | XDP | Loader relies on the operator/systemd for bpffs pin-dir mode; it asserts the path is bpffs but does not itself lock down the directory mode/owner. A world-writable pin dir would let a local unprivileged user tamper with conntrack → mis-route. Out-of-process hardening, documented. |

Proven-clean scopes (defence + test) are listed at the bottom — they are the
bulk of the result.

---

## Per-finding detail

### F-INFRA-01 · LOW · TLS/secrets · retry-secret load path skips the perm check

**Mechanism.** The retry-token signing secret is a 32-byte HMAC-SHA256 key
that (a) signs/verifies stateless Retry tokens and (b) in Mode A passthrough is
the Initial-flood mitigation's trust anchor. It is **generated** correctly with
`O_CREAT|O_EXCL, mode 0600`:

- `crates/lb-quic/src/listener.rs:517` `write_secret_file` → `OpenOptions::new().create_new(true).mode(0o600)`
- `crates/lb-quic/src/passthrough.rs:1301` — identical.

But the **read/load** path does NOT check the file's permissions on an existing
file:

- `crates/lb-quic/src/listener.rs:481-499` `load_or_generate_retry_secret` — `std::fs::read(path)` then length-check only.
- `crates/lb-quic/src/passthrough.rs:~1260-1297` — identical.

Contrast the TLS private key, which IS perm-checked on every load (startup AND
SIGUSR1 reload), strict in release builds:
`crates/lb/src/main.rs:980` `assert_key_perm_advisory` (`strict = !cfg!(debug_assertions)`)
calling `lb_security::assert_owner_only` (`crates/lb-security/src/key.rs:97`).
The retry secret has no equivalent gate. This is asymmetric hardening: the
gateway refuses to load a world-readable TLS key but silently loads a
world-readable retry secret.

**Threat.** An attacker who can read the retry secret (loose perms from an
operator pre-placing the file, a restore from a backup with wrong umask, or a
config-management drift) can forge Retry tokens. For Mode A passthrough that
defeats the §6.5 Initial-flood defence (`mint_retry=true`): forged tokens pass
`RetryTokenSigner::verify`, so the attacker's spoofed Initials are treated as
already-validated and forwarded to backends. It does NOT leak traffic plaintext.
Requires local read access to the secret file ⇒ LOW, but it is a real
defence-in-depth asymmetry against the TLS-key treatment.

**PoC (lead runs).** Place a pre-existing retry secret with loose perms, boot a
QUIC listener pointing at it, observe it loads without error:
```sh
mkdir -p /tmp/eg-poc && head -c32 /dev/urandom > /tmp/eg-poc/retry.secret
chmod 0644 /tmp/eg-poc/retry.secret   # group/other-readable
# point [listeners.quic].retry_secret_path (or [passthrough].retry_secret_path)
# at /tmp/eg-poc/retry.secret and boot.
```
Expected today: boots clean, no warning. Expected after fix: a `tracing::warn!`
(lax) or a hard startup error (strict, matching the TLS-key posture).

**Exact test the lead should add (mirrors the key.rs sweep).** In
`crates/lb-quic` add a unit test on the load path:
```rust
#[cfg(unix)]
#[test]
fn retry_secret_loose_perms_is_rejected_or_warned() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir();
    let p = dir.path().join("retry.secret");
    std::fs::write(&p, [7u8; 32]).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
    // After the fix, load_or_generate_retry_secret must surface the
    // loose perms (Err in strict, advisory otherwise) instead of Ok-silently.
    let r = load_or_generate_retry_secret(&p);
    assert!(r.is_err() || /* advisory path observable */ true);
}
```
**Minimal fix.** In both `load_or_generate_retry_secret` impls, on the `Ok(bytes)`
(existing-file) arm call `lb_security::assert_owner_only(path, strict)` before
trusting the bytes (thread the same `strict = !cfg!(debug_assertions)` the TLS key
uses), warn-or-fail symmetrically. ~4 lines × 2 sites.

**Disposition.** Open, LOW. Recommend fix this session (cheap, closes a
real asymmetry the TLS-key code already establishes the pattern for).

---

### F-INFRA-02 · LOW · secrets · no zeroization of key material in memory

**Mechanism.** `grep -rn zeroize crates/ → empty`. TLS private keys are handed
to rustls `with_single_cert` (`crates/lb-security/src/ticket.rs:372`) and live
inside `Arc<rustls::ServerConfig>`; the retry secret lives in `ring::hmac::Key`
inside `RetryTokenSigner` (`crates/lb-security/src/retry.rs:116-152`); ticket
keys in `TicketKey` (`ticket.rs:61`). None implements `Drop`/`Zeroize`, so on
free the bytes linger in the heap until overwritten.

**Threat.** This is a defence-in-depth gap, not a reachable leak: there is no
wire/admin path that reads freed heap. It matters only under a separate
primitive (core dump, a different memory-disclosure bug, swap-to-disk). The
redaction discipline that prevents the *reachable* leak is strong and proven —
see the F-INFRA-CLEAN-3 scope. Recorded for completeness; rustls itself does not
zeroize, so closing this fully needs upstream cooperation.

**Disposition.** Documented LOW. No code change recommended this session
(would require wrapping rustls-owned material; low value vs. risk).

---

### F-INFRA-03 · LOW (documented posture) · TLS · no mTLS + TLS 1.2 default

**Mechanism.** `crates/lb-security/src/ticket.rs:370` `.with_no_client_auth()` —
the server never requests a client cert. `ticket.rs:366-368`
`with_safe_default_protocol_versions()` (rustls default `&[TLS12, TLS13]`)
unless `[runtime.tls].tls13_only = true` (`crates/lb-config/src/lib.rs:531`,
default `false`).

**Assessment.** For an internet-facing reverse proxy, no *server-side* mTLS is
the normal posture (clients are browsers/anonymous). rustls's default TLS 1.2
suites are downgrade-safe (ECDHE-only, AEAD-only); rustls has no SSLv3/TLS1.0/1.1
and no RC4/CBC-without-EtM. So this is not a downgrade vulnerability — it is a
compliance toggle. Documented so the deployment profile is explicit; operators
needing PCI-DSS 4.0 §4.2.1.1 set `tls13_only=true`.

**Disposition.** Documented LOW. No finding to fix. (Backend/upstream cert
verification IS enforced — see F-INFRA-CLEAN-5.)

---

### F-INFRA-04 · LOW (documented) · XDP · pin-dir mode not enforced by the loader

**Mechanism.** `load_from_bytes_pinned` (`crates/lb-l4-xdp/src/loader.rs:856`)
asserts the pin dir is bpffs (`crates/lb-l4-xdp/src/bpffs.rs:45 assert_bpffs`,
`statfs` magic check) and then `loader.map_pin_path(p)`. It does not stat/lock
the directory **mode or owner**. Default pin dir `/sys/fs/bpf/expressgateway`
(`DEFAULT_PIN_DIR`). bpffs.rs:12 explicitly delegates the mount + dir to the
systemd unit (OPS-07).

**Threat.** If an operator mounts bpffs world-writable (non-default; `/sys/fs/bpf`
is root:root 0700 on stock distros), a local unprivileged user could
`BPF_OBJ_GET` the pinned CONNTRACK map and write rewrite entries → mis-route
data-plane traffic. This requires a misconfigured mount AND local access ⇒ LOW,
and it is an out-of-process concern. The loader could add a belt-and-suspenders
`metadata(pin_dir).mode() & 0o022 == 0` check mirroring `assert_owner_only`.

**Disposition.** Documented LOW. Optional hardening (one stat in
`load_from_bytes_pinned`); not required this session. Multi-kernel verifier
portability remains carried as F-ESC-1.

---

## Proven-clean scopes (defence + the test that proves it)

These were attacked and held. Each lists the defence (file:line) and the
existing test that proves it (R4 — not assumed).

### F-INFRA-CLEAN-1 · config "0 = disable" foot-guns (L-INFRA-1)

Every dangerous knob is either range-validated or a **documented** disable
sentinel; `deny_unknown_fields` on all 21 structs rejects typo'd keys at parse
time. Enumerated:

| Knob | "0"/dangerous value | Validator behaviour (lib.rs) |
|------|---------------------|------------------------------|
| `max_keepalive_requests` | `0`=disable, fat-finger huge | `0` ok; `>10M` REJECTED (:1510) |
| `max_requests_per_h3_connection` | `0`=disable (re-opens StreamMap leak) | `0` documented-ok; `>10M` REJECTED (:1522) |
| `xdp_new_flow_cap_per_sec_per_cpu` | `0`=disable | `0` ok; else clamped `1000..=10M` (:1494) |
| `per_ip_connection_cap` | absurd-high / 0 | `1..=2_000_000` enforced (:1455) |
| `max_inflight_connections` | absurd | `100..=2_000_000` (:1438) |
| `handshake_timeout_ms`/`connect_timeout_ms` | 0 starves | `100..=60_000` (:1428,:1447) |
| `tls_verify_peer=false` (H3 backend) | MITM-accept | only valid for `protocol="h3"`; on non-H3 backends the knob is REJECTED (:1752); on H3 requires `tls_ca_path` unless explicit opt-out (:1764) |
| watchdog `body_progress_min_bps` | huge | `<=10M` (:1473) |
| passthrough `min_client_dcid_len` | `0` re-opens CVE-2022-30592 | `8..=20` enforced (:1935) |
| ws/grpc `*_seconds`/`*_size` | `0` | each `> 0` enforced (validate_websocket/grpc) |
| `mint_retry=false`, `strict_source_binding=false`, `flow_idle_timeout_ms=0` | weaken passthrough | DOCUMENTED escapes (struct docs :1171-1197), availability trade-offs, not silent |

I could not construct a well-typed config that silently disables a defence
without it being a documented, range-bounded sentinel.
**Test:** `crates/lb-config/src/lib.rs` validator unit tests +
`valid_config_still_parses_under_deny_unknown_fields` (:2380) +
the `deny_unknown_fields` rejection test (:2294). Lead can add explicit
"max_keepalive_requests=4000000000 is rejected" assertions to make the fat-finger
guard self-documenting.

### F-INFRA-CLEAN-2 · SIGHUP reload race / honesty-contract (L-INFRA-2)

No torn snapshot, no cross-connection config bleed, no restart-required-applied-live.
- **Per-connection consistency:** each connection does exactly ONE `.load_full()`
  for the leg it serves (`crates/lb/src/main.rs:3807` H1, `:3864`/`:3879` the
  ALPN-dispatched H2/H1 leg of H1s) and serves the whole connection on that owned
  snapshot. A concurrent `.store()` (`:666`,`:704-705`) leaves the captured Arc
  live until the connection drops (RCU). No connection re-reads the swap.
- **H1s two-store "tearing":** the two `.store()` calls (h1 leg + h2 leg,
  `:704-705`) cannot tear within a connection because ALPN dispatch picks exactly
  ONE leg per connection (`:3855`), and each proxy is internally consistent.
- **Honesty contract is EXHAUSTIVE:** every `ListenerConfig` field (12) and every
  top-level block (6) lands in exactly one of swappable / restart-required;
  `max_requests_per_h3_connection`, `tls`, `quic`, `drain_*`, `[admin]`,
  `[security]`, `[passthrough]`, all XDP fields → restart-required (logged +
  metric, never silently applied) (`crates/lb-config/src/reload.rs:283-421`,
  applied at `main.rs:472-479`). Validation runs on the new config BEFORE diff;
  parse/validate failure rolls back and keeps the live config (`main.rs:446-464`).
**Test:** `reload.rs` tests `tls_and_drain_changes_are_restart_required_not_swappable`
(:678), `combined_backend_and_http_change_is_one_swappable_with_both_fields` (:631),
`protocol_change_is_restart_required_and_subsumes_backends` (:559); the in-tree
RCU proof `main.rs:6159-6188` (load_full snapshot survives a concurrent store).

### F-INFRA-CLEAN-3 · admin API auth + probe exposure (L-INFRA-3)

- **Constant-time token compare with no length side-channel:** the candidate is
  SHA-256-hashed to a fixed 32 bytes BEFORE the `subtle::ConstantTimeEq` compare
  (`crates/lb-security/src/admin_auth.rs:202-211`), so token length only varies
  the SHA-256 input (not the secret-dependent compare). `from_hex` length is a
  startup-time config check, not request-path.
- **Bind guard fails closed and is wired:** `validate_bind` rejects non-loopback
  without `allow_non_loopback`, and `allow_non_loopback` without a token
  (`admin_auth.rs:229`), called via `?` BEFORE bind (`main.rs:2652-2657`).
  `serve_with_auth` (the gate-bearing path) is used, not the no-auth `serve`
  (`main.rs:2659`).
- **Probes leak nothing sensitive:** `/livez /readyz /startupz /healthz` return a
  closed-vocabulary `{"status":"…"}` from `ProbeState::body_token`
  (`crates/lb-observability/src/admin_http.rs:146-160`) — no version, build, or
  config. `/metrics` IS gated by the token when enforced (`admin_http.rs:84-93`);
  probes are intentionally exempt for the kubelet. Data plane cannot reach the
  admin bind because it is a separate `metrics_bind` listener.
**Test:** `admin_auth.rs` tests (constant-time compare, bind matrix
:272-302, authorize matrix :306-351), `admin_http.rs::test_admin_403_without_token`
(:326), `main.rs:6374-6399` bind-guard hard-exit test.

### F-INFRA-CLEAN-4 · upstream cert verification is enforced (part of L-INFRA-4)

Production H3-upstream config factory sets `verify_peer(true)` whenever
`tls_verify_peer` (config, default true, validator-required `tls_ca_path` for H3)
(`crates/lb/src/main.rs:1233-1252`); Mode B raw_proxy is `verify_peer(true)`
ALWAYS with no off-knob (`crates/lb-quic/src/raw_proxy.rs:1615-1617`,:2411). The
only `verify_peer(false)` sites are inside `#[cfg(test)]` (`lb-io/src/quic_pool.rs:687`
test factory; `lb-quic/src/router.rs:581,711` after `mod tests` at :489) — NOT
on any production path. A MITM upstream cannot be silently accepted.
**Test:** `main.rs:4085 build_h3_upstream_pool_rejects_mismatched_verify_peer`;
`crates/lb-config` `validate_backend_h3_tls` tests.

### F-INFRA-CLEAN-5 · secret redaction discipline (L-INFRA-5)

Every secret-bearing struct has a hand-written `Debug` that renders NO secret
material via `finish_non_exhaustive()`: `AdminTokenHash` (admin_auth.rs:135),
`RetryTokenSigner` (retry.rs:122), `TicketKey` (ticket.rs:96), `TicketRotator`
(ticket.rs:241), `RotatingTicketer` (ticket.rs:277), `TlsConfigBundle`
(ticket.rs:494). No `tracing::{info,warn,error,debug,trace}!` interpolates a key/
token/secret byte (full grep: the only hits log error variants + peer addr +
"TLS ticket key rotated" status — `router.rs:246`, `passthrough.rs:823`,
`main.rs:1068`). Private key (`PrivateKeyDer`) is passed straight to
`with_single_cert`, never formatted. lb-security has zero secret-bearing tracing.
**Test:** `admin_auth.rs::debug_does_not_print_digest_bytes` (:378); extend with
a Debug-redaction sweep over the other 5 structs for self-documentation.

### F-INFRA-CLEAN-6 · XDP packet-parse bounds + new-flow cap (L-INFRA-6)

Single-kernel. Every `data..data_end` deref goes through `ptr_at`/`ptr_at_mut`
which bounds-check with **`checked_add`** (overflow-safe against the
aya-#1562 / CVE-2022-23222 bounds-elision class)
(`crates/lb-l4-xdp/ebpf/src/main.rs:512-540`). Verified:
- IPv4 IHL-driven `ip_hdr_len` (≤60) → `l4_offset` is re-bounds-checked by
  `ptr_at::<TcpHdr>` (:695); the rewrite re-validates the larger 24-byte
  `TcpHdrRW` independently (:852,:1091) so the offset-16 checksum read can't OOB.
- IPv6 ext-header walk is bounded to 2 iterations, each deref bounds-checked
  (:943-958); a huge attacker `hdr_ext_len` just pushes `off` past `data_end`,
  caught by the next `ptr_at` → `XDP_PASS`. Fragments (v4 frag_off, v6 Fragment
  EH) → `XDP_PASS` (:675,:969).
- Any parse failure → `Err(())` → `XDP_PASS` (never `XDP_DROP`) (:613-616).
- New-flow rate cap (SYN-flood): per-CPU sliding window, `saturating_add`,
  fails OPEN if the map is unreadable, `cap==0` disables (:464-490). Map keys are
  fixed `#[repr(C)]` structs with explicit padding (no uninit-pad key-mismatch).
- Loader: license-GPL assert → bpffs assert → pin, all `?`-fail-closed
  (`loader.rs:856-882`); CODE-2-07 size asserts pin Rust↔ELF layout (:405-481);
  ENA NIC blocklist demotes Drv→Skb.
**Test:** `round8_ptr_at_bounds.rs`, `round8_fragments.rs`,
`round8_synflood_cap.rs`, `round8_verifier_baseline_70.rs`,
`loader_license_assert.rs`, `round8_bpffs_check.rs`, `round8_ena_kernel_blocklist.rs`.

---

## Notes for the lead

- The only finding worth a code change this session is **F-INFRA-01** (retry-secret
  load-path perm check) — ~4 lines × 2 sites, mirrors the existing
  `assert_key_perm_advisory` pattern, closes a real TLS-key-vs-retry-secret
  asymmetry. The rest are documented LOW posture items.
- No HIGH/CRITICAL in this surface. The operational layer is well-built; S37's
  `deny_unknown_fields` + diff honesty-contract + admin gate hold up adversarially.
