# Round 6 — `sec` adversarial cross-check of `proto`'s Round-6 mediums

Reviewer:    `sec`
Branch:      `prod-readiness/round-4` @ `526c1bc`
Scope:       PROTO-2-18 (`444668d`) + PROTO-2-19 (`ed35fdb` — author-SHA
             `42df990`); plus a final delta sweep on every commit after
             `1f2e19c` (the Round-6 sec sweep) up through `526c1bc`.
Proto self-verification: `audit/protocol/round-6-revalidation.md`.
Proto findings:           `audit/protocol/round-6-delta-findings.md`.
Toolchain:   cargo 1.85.1.
Sanity:      `cargo build -p lb-l7 -p lb -p lb-config` — PASS (cache hit).
             `cargo test -p lb-l7 --test sni_authority_421
              --test trailer_passthrough` — 8 / 8 PASS (the two PROTO-2-18
             real-TLS tests + the six PROTO-2-19 wire-emit / bridge tests).
             Workspace-wide `cargo test -p lb-l7` filled the disk
             during link of unrelated test binaries; the two targeted
             tests already completed PASS before that — not a regression
             of the round-6 fixes.

---

## Part 1 — Adversarial cross-check

### PROTO-2-18 — SNI ↔ `:authority`/Host wiring

Implementation under review:
* `crates/lb-l7/src/sni_authority.rs::check_sni_authority` (Round-5
  validator; unchanged).
* `crates/lb-l7/src/h1_proxy.rs:617-647` — H1 hot-path call site after
  `hooks.inspect_request`.
* `crates/lb-l7/src/h2_proxy.rs:523-558` — H2 hot-path call site after
  PROTO-2-01 `check_authority_host_agreement`.
* `crates/lb/src/main.rs:2363,2428,2447,2460` — accept-site SNI
  capture via `tls_stream.get_ref().1.server_name().map(str::to_owned)`
  threaded into `serve_connection_with_cancel_sni`.

#### Bypass attempt 1 — Case games (`SNI=example.com`, `Host=EXAMPLE.com`)

`check_sni_authority` calls `normalise_host` on both sides, which lower-
cases via `to_ascii_lowercase`, then compares with `eq_ignore_ascii_case`.
The double normalisation is redundant but the comparison is case-
insensitive in either path. Mixed-case SNI vs mixed-case Host accepts.

Verdict: **safe; case-insensitive accept**. Confirmed by unit test
`case_insensitive_match` in `sni_authority.rs::tests`.

#### Bypass attempt 2 — Trailing dot per DNS (`SNI=example.com.`, `Host=example.com`)

`normalise_host` calls `trim_end_matches('.')` on both sides. So
`example.com.` ↔ `example.com` accepts. Either-side trailing dot is
normalised. Confirmed by unit test `trailing_dot_normalised`.

Verdict: **safe; trailing-dot tolerant accept** (the canonical DNS form).
Policy is consistent with browser behaviour.

#### Bypass attempt 3 — Port in SNI (`SNI=example.com:443`)

This case is **impossible at the rustls boundary**. The hot-path SNI
value comes from `rustls::server::ServerConnection::server_name()`,
which returns a parsed `DnsName<'_>` (rustls-pki-types). `DnsName`'s
`TryFrom<&str>` rejects any input containing `:` (it is enforced to be
a syntactically valid DNS name per RFC 1035 + RFC 5890; ports / schemes
are syntactically out-of-grammar). So `expected_sni` can never carry
a port.

The author's `check_sni_authority` does *not* call `split_host_port`
on the SNI side (only on `authority`), which would matter if a future
caller hand-fed a port-bearing SNI. That is a hardening note for the
follow-up audit (see INFO-CC-1 below) but is not reachable on the
production hot path today.

Verdict: **safe via upstream rustls grammar; not reachable**.

#### Bypass attempt 4 — SNI present but request `:authority` / Host empty

`check_sni_authority` returns `Ok(())` when `authority.is_empty()` —
i.e. the SNI check no-ops, deferring to PROTO-2-01 for the
missing-authority gate.

On H2, hyper requires `:authority` to be present for non-CONNECT
requests; the inbound `uri.authority()` is populated. If a client
manages to send an empty `:authority` and no `Host`, hyper itself
typically rejects at H2 frame parse. Even if it slips through,
`check_authority_host_agreement` returns Ok (one side missing) and
`check_sni_authority` also returns Ok (authority empty). The PROTO-2-01
empty-authority gap **predates** this round and is tracked separately;
PROTO-2-18 is a co-defence, not the primary gate, so passing on empty
is the documented contract (see `sni_authority.rs:98-101`).

On H1, hyper rejects requests without a `Host` header at HTTP/1.1 per
RFC 9112 §3.2 + RFC 9110 §7.2 (`hyper::Error::Parse(Header)`). A blank
`Host:` value reaches the proxy as `authority = ""`, which the SNI
gate intentionally lets through; PROTO-2-01 / hyper's parser handle
that case upstream.

Verdict: **safe-by-design no-op when authority absent**; the documented
upstream gate is responsible. Not a new bypass introduced by PROTO-2-18.

#### Bypass attempt 5 — Loopback exception with IPv4-mapped IPv6 (`::ffff:127.0.0.1`)

I confirmed empirically (rustc playground):

| address                | `is_loopback()` (Rust std) |
|------------------------|---------------------------|
| `127.0.0.1`            | `true`                    |
| `::1`                  | `true`                    |
| `::ffff:127.0.0.1`     | **`false`**               |
| `::ffff:8.8.8.8`       | `false`                   |

`Ipv6Addr::is_loopback()` matches **only** `::1`. The IPv4-mapped form
of any address is not loopback per stdlib. So if a dual-stack listener
surfaces a v4 loopback connection as `SocketAddr::V6` with
`::ffff:127.0.0.1`, the SNI/Host check **DOES fire** (the exception
does **not** trigger).

This is **safe-by-default strictness** — the gate errs toward
enforcement, which is the right direction for an authz check. It can
only over-reject a legitimate same-host loopback probe if the listener
binds to `[::]` AND the OS lands the v4 loopback as a v4-mapped v6
socket. On Linux with default `bindv6only=0`, that is the actual
behaviour (`accept()` returns `AF_INET6` with a v4-mapped sender for
v4 clients). A v4-loopback probe arriving at the dual-stack listener
will therefore go through the full SNI/Host check — which is **fine
when the probe is well-formed** (matching SNI/Host). The same
defensive observation lives in `audit/security/round-6-delta-findings.md`
under the SEC-2-06 review (`Ipv6Addr::is_loopback()` is exactly `::1`).

Verdict: **not a bypass — strictness**. Recommend updating the
PROTO-2-18 rustdoc to call out the v4-mapped v6 caveat so an operator
who hits a probe-script regression has a one-grep diagnostic; tracked
as INFO-CC-2 below. No code change required.

#### Bypass attempt 6 — `:authority` vs `Host` disagree on H2 (which fires first?)

In `H2Proxy::handle` (`h2_proxy.rs`):

1. `check_authority_host_agreement(&parts.uri, &parts.headers)` — line 518.
   On disagreement returns `Err(msg)` → `400 BAD_REQUEST` immediately.
2. `check_sni_authority(expected_sni, authority)` — line 547.
   Reached only after step 1 has passed.

So when both `:authority` and `Host` are present and disagree, PROTO-2-01
fires first (400), not PROTO-2-18 (421). Correct precedence per the
plan's "smuggle → :authority/Host → SNI/Host" ordering. The chosen
`authority` value at step 2 is `:authority` first, `Host` second —
matching the documented contract.

Verdict: **safe; precedence correct**.

#### PROTO-2-18 summary

Six bypass attempts, zero exploitable. The hot-path wiring matches
the validator's contract; the loopback exception is conservative
(strict on v4-mapped v6); RFC-9110 §15.5.20 response code and phrase
match canonical form (verified by `test_421_emitted_on_sni_host_mismatch_over_tls`).

---

### PROTO-2-19 — H1 trailer wire emission

Implementation under review:
* `crates/lb-l7/src/h1_proxy.rs:1371-1434` — `build_h1_response_with_trailers`.
* `crates/lb-l7/src/h1_proxy.rs:1271-1299` — `build_body_with_trailers`.
* Bridge layer: `crates/lb-l7/src/h2_to_h1.rs:128-153` — forwards
  `resp.trailers` unfiltered (intentional — hyper's H1 encoder
  applies the §6.5.2 forbidden-trailer filter; see below).

#### Bypass attempt 1 — Trailer name containing CRLF / NUL / colon

`build_body_with_trailers` (line 1284-1291) wraps each `(name, value)`
in `(HeaderName::try_from(name), HeaderValue::from_str(value))`. Both
constructors reject the structural-injection bytes:

* `HeaderName::try_from` rejects any byte outside the `tchar` set
  (RFC 9110 §5.6.2) — `\r`, `\n`, `:`, `0x00..=0x1f`, `0x7f..=0xff`,
  `' '`, `'\t'`, `'"'`, etc.
* `HeaderValue::from_str` rejects `\r`, `\n`, `\0`, and any non-visible
  ASCII outside the obs-text range.

When either rejects, the silent `if let (Ok, Ok)` branch drops the
pair entirely — no panic, no partial insertion. The `Trailer:` header
declaration, however, is built earlier from the **raw `translated.trailers`
names** (line 1404-1408) via `HeaderValue::from_str(&trailer_header)`.
If the upstream trailer name contains a comma in the name itself,
the `Trailer:` value would split across two declared trailers; but
hyper's `HeaderName::try_from` would still reject `,` in the name, so
the actual trailer frame is dropped and the `Trailer:` declaration
just enumerates a name that never appears on the wire. RFC 9110 §6.6.2
permits a `Trailer:` declaration without a matching trailer; clients
parsing strictly may warn, but no smuggling primitive results.

CRLF injection into a trailer name: rejected at `HeaderName::try_from`.

Verdict: **safe; HeaderName / HeaderValue grammar gates all
structural-injection bytes**.

#### Bypass attempt 2 — `Trailer:` lists a name absent from the actual trailers

In our code path the `Trailer:` declaration is **derived from**
`translated.trailers` (line 1404-1408), so the list is always a
superset-or-equal of the actually emitted trailer frame. The only way
a name appears in the declaration but not on the wire is if hyper's
encoder filters that specific name via `is_valid_trailer_field` (e.g.
the upstream sent `content-length` as a trailer — see attempt 3). The
chunked terminator `0\r\n\r\n` is still emitted cleanly by hyper's
encoder (`encode.rs:163-213`); the trailer-block is well-formed even
when the trailer HeaderMap that survives filtering is empty (the
empty-trailers branch writes the terminator + the empty trailing
fields block).

Verdict: **safe; chunked terminator emitted regardless**.

#### Bypass attempt 3 — Forbidden trailer per RFC 9110 §6.5.2 (e.g. `content-length`)

The proxy's `build_h1_response_with_trailers` does **NOT** strip
RFC-9110-§6.5.2-forbidden names from the inbound `translated.trailers`
vec before either (a) constructing the `Trailer:` declaration or (b)
handing the trailer HeaderMap to hyper.

However, hyper-1's H1 encoder defends in depth. `hyper-1.9.0/src/proto/
h1/encode.rs::is_valid_trailer_field` (line 264-280) **drops** any
trailer whose `HeaderName` matches the §6.5.2 forbidden set:

  `AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH,
   CONTENT_RANGE, CONTENT_TYPE, HOST, MAX_FORWARDS, SET_COOKIE, TRAILER,
   TRANSFER_ENCODING, TE`

For each trailer in the HeaderMap, hyper additionally requires the
name to be **listed in the response's `Trailer:` declaration** (line
170-189) — if not declared, the trailer is dropped with a debug log.

So an upstream that smuggles `transfer-encoding: chunked` as a trailer
in its H2 response gets:

1. The byte sequence inserted into our `translated.trailers` vec.
2. The name `transfer-encoding` added to our `Trailer:` declaration
   on the H1 response head (this **is** a spec issue — RFC 9110 §6.6.2
   says `Trailer:` SHOULD only contain valid trailer fields).
3. Hyper drops the actual trailer frame at encode time; the wire
   carries the head `Trailer: transfer-encoding` declaration **but no
   matching trailer**.

The downstream H1 client that strictly parses `Trailer:` and treats
its declarations as authoritative could be confused into double-
framing or `transfer-encoding` reinterpretation. This is the same
class of issue PROTO-2-12's existing recommendation #4 already tracks
("Strip hop-by-hop names from trailers as well — `Trailer` itself,
plus `Content-Length`, `Cache-Control`, etc. forbidden as trailer
names per RFC 9110 §6.6.1"). The Round-6 revalidation note on
PROTO-2-12 acknowledges this gap: *"Forbidden-trailer-name strip
remains a follow-up plan"*.

Real-world exploitability requires (a) a compromised / malicious
upstream backend and (b) a downstream that re-frames based on the
`Trailer:` declaration string rather than actually-received trailer
frames. Both are unusual. The wire **never carries an actual
forbidden trailer field** thanks to hyper's `is_valid_trailer_field`
defence — so this is not a smuggling primitive against a spec-
compliant downstream.

Verdict: **already-tracked spec-compliance gap; not a new bypass
introduced by PROTO-2-19**. Recorded as INFO-CC-3 for the existing
PROTO-2-12 follow-up.

#### Bypass attempt 4 — H2 client `te: trailers` → H1 upstream → trailer response: TE strip on req side, Trailer add on resp side?

Request-side strip: `crates/lb-l7/src/h1_proxy.rs:69-78` declares
`HOP_BY_HOP` including `te`; `strip_hop_by_hop` removes it. The H2-to-H1
request bridge (`h1_to_h2.rs` / inverse) honours the same set. So the
upstream H1 request the gateway emits does **not** carry the client's
`TE: trailers` header — the strip is correct.

Response-side add: `build_h1_response_with_trailers` injects
`Trailer:` + `Transfer-Encoding: chunked` only when
`translated.trailers` is non-empty (the `has_trailers` flag, line
1377). When the upstream H1 response carries no trailer block, no
Trailer/TE-chunked injection happens, and the response keeps its
upstream framing.

The spec corner: RFC 9110 §6.6.1 — *"A server MUST NOT generate a
trailer field unless the client indicates willingness via `TE:
trailers`"*. Our H1 listener emits trailers regardless of the inbound
`TE:` value. This is a behaviour gap, not a smuggling primitive:
RFC-compliant clients that did **not** signal `TE: trailers` will
likely buffer the chunked body, ignore the trailer fields, and
complete the request. Hyper's H1 client, for example, simply drops
unrequested trailers. No new attack surface.

Verdict: **safe; TE strip correct on request side, optional-trailer
emit on response side is RFC-non-strict but not a smuggling
primitive**. Filed as INFO-CC-4 for a future hardening pass.

#### PROTO-2-19 summary

Four bypass attempts, zero exploitable. The header / value grammars
gate structural injection; hyper's H1 encoder applies the §6.5.2
forbidden-trailer filter defensively; chunked terminator is well-
formed in all branches. Two pre-existing spec-compliance gaps
(forbidden-trailer-name strip at the proxy layer; `TE: trailers`-
gated emit) are already known to PROTO-2-12 follow-up and recorded
again here as info-level cross-check observations.

---

### Informational observations (sub-low; not regression-worthy)

#### INFO-CC-1 — `check_sni_authority` does not split a port out of the `sni` argument
File: `crates/lb-l7/src/sni_authority.rs:95-113`
Severity: info
The validator strips `host[:port]` from the `authority` argument but
not from the `sni` argument. Today the only caller is the rustls
accept-site, which can never produce a port-bearing SNI (`DnsName`
rejects `:` at parse time). A future caller (e.g. propagating SNI via
PROXY-protocol v2 TLV) could violate this implicit invariant.
Recommendation: apply `split_host_port` symmetrically on both sides so
the validator is robust against any future caller. Two lines of code.

#### INFO-CC-2 — IPv4-mapped IPv6 loopback is not stdlib-loopback
File: `crates/lb-l7/src/h1_proxy.rs:629`, `h2_proxy.rs:534`
Severity: info
`Ipv6Addr::is_loopback()` is **exactly** `::1`. A dual-stack listener
on Linux (default `bindv6only=0`) surfaces `127.0.0.1` clients as
`::ffff:127.0.0.1`, for which `IpAddr::is_loopback() == false`. SNI
enforcement therefore fires for v4-loopback probes that reach a v6
listener — strict by direction (which is good) but surprising at
debug time. Recommendation: append a one-line caveat to the existing
rustdoc block (lines 617-628) noting the v4-mapped-v6 behaviour and
suggesting `IpAddr::to_canonical().is_loopback()` (nightly) or an
explicit `Ipv6Addr::to_ipv4_mapped()` short-circuit for operators
who want to allow the v4-loopback probe over a dual-stack bind.

#### INFO-CC-3 — Forbidden-trailer-name strip absent at the proxy layer
File: `crates/lb-l7/src/h1_proxy.rs:1271-1299` (and the H2/H3 bridge
trailer-forward sites).
Severity: info (duplicate of PROTO-2-12 follow-up rec #4)
Hyper's H1 encoder defends in depth, so the wire emission stays safe
even when the upstream sneaks a `content-length` trailer; only the
head-level `Trailer:` *declaration* leaks the forbidden name. This is
the same gap PROTO-2-12 already tracks. No new severity.

#### INFO-CC-4 — H1 trailer emit not gated on inbound `TE: trailers`
File: `crates/lb-l7/src/h1_proxy.rs:1377` (the `has_trailers` branch).
Severity: info
RFC 9110 §6.6.1 requires the server to gate trailer emission on the
client's `TE: trailers` token. The current code emits trailers
whenever the bridge supplies them, regardless of the client's `TE:`.
RFC-compliant clients tolerate this (they drop unrequested trailers);
some legacy parsers may treat the wire bytes as protocol error.

---

## Part 2 — Final delta sweep

Commits introduced after `1f2e19c` and up through `526c1bc` (current
HEAD on `prod-readiness/round-4`):

| SHA       | Class           | Summary                                                                                  |
|-----------|-----------------|------------------------------------------------------------------------------------------|
| `8444211` | audit (code)    | Round-6 code delta-discovery sweep — `audit/code/round-6-delta-findings.md` (no code).   |
| `752601f` | proto (Low)     | PROTO-2-16 — H1 cancel rustdoc fix-up; **doc-only**, no code change, no Cargo.lock churn. |
| `603a6f6` | proto (Low)     | PROTO-2-17 — `[security].strict_te` knob; adds opt-in stricter H1 TE policy. **Tightens**, never loosens. |
| `69a774a` | proto (Medium)  | PROTO-2-18 — `check_sni_authority` wiring; new 421-Misdirected-Request gate.             |
| `ed35fdb` | proto (Medium)  | PROTO-2-19 — H1 trailer wire emission; closes a silent-drop bug.                         |
| `526c1bc` | audit (proto)   | Round-6 self-verification + Status flips; no code.                                       |

#### Cargo.lock / dependency delta

`crates/lb-l7/Cargo.toml` grew **`[dev-dependencies]`** for the PROTO-2-18
real-TLS test (`tokio-rustls`, `rustls`, `rustls-pki-types`,
`rcgen 0.13` with `pem`). These are dev-only and do not ship in the
release binary; verified by inspecting the section heading in the
diff. No new runtime dependency, no Cargo.lock CVE-relevant version
churn (the runtime resolutions for `rustls`, `rustls-pki-types`,
`tokio-rustls` were already present via the `lb-security` ticket
path).

#### Attack-surface delta

* **PROTO-2-17** — adds one boolean (`config.security.strict_te`)
  routed into the `SmuggleMode` selection in `main.rs`. Default
  `false` preserves the lenient baseline (no behaviour change unless
  the operator opts in). Opt-in path is RFC-9112-§7.1 strict TE
  rejection — pure tightening, no new surface.
* **PROTO-2-18** — adds the SNI propagation field (`expected_sni:
  Option<String>`) to `H1Proxy` / `H2Proxy` plus the
  `with_expected_sni` builder and `serve_connection_with_cancel_sni`
  entry. The new code path **runs after** PROTO-2-01 + smuggle
  detection on every TLS-terminated listener and **only adds reject
  decisions** (421 on mismatch). No new accept path, no header
  generation, no upstream surface change. Loopback exception is a
  reject-side carve-out (skips the gate), not an accept carve-out.
* **PROTO-2-19** — flips the H1 response head shape from
  Content-Length-or-untouched to `Transfer-Encoding: chunked +
  Trailer:` when the bridge supplied trailers. The `has_trailers`
  branch (line 1377) is **only** taken when `translated.trailers` is
  non-empty; the no-trailer path is byte-identical to the pre-fix
  behaviour. Hyper's `is_valid_trailer_field` filter (encode.rs:264)
  is the floor on what reaches the wire — no new structural-injection
  surface introduced.

#### Conclusion

Zero new medium-or-higher findings introduced by any of the
six post-`1f2e19c` commits. The PROTO-2-18 / PROTO-2-19 fixes
are tightening / closing existing gaps and do not widen any attack
vector. The two new informational observations (INFO-CC-2 v4-mapped
v6 loopback caveat; INFO-CC-4 TE-gated trailer emit) are doc-/policy-
level and do not warrant re-entry to Round 3.

---

## Verdict

**Round 6 — CLEAN; ready for Round 7.**

* PROTO-2-18 cross-checked: six bypass attempts, zero exploitable;
  precedence correct (PROTO-2-01 before PROTO-2-18); rustls
  enforces SNI grammar at the boundary; loopback exception is
  conservative on v4-mapped v6.
* PROTO-2-19 cross-checked: four bypass attempts, zero exploitable;
  hyper's encoder applies the RFC 9110 §6.5.2 forbidden-trailer
  filter; HeaderName/HeaderValue grammars block structural
  injection; TE strip + Trailer add correct.
* Final delta sweep: zero new medium-or-higher attack-surface
  changes since `1f2e19c`.

No `audit/security/round-6-bypass-findings.md` is opened. Round 6
closure proceeds.
