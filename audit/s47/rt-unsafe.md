# S47 — Red-team pass: `unsafe` soundness + panic reachability

Branch `review/s47-rfc-security` (main @ 01915a77). Read-and-reason only; **no cargo
command was run** (2 vCPU / 7 GB / 11 GB free — per the engagement rule). Every claim
below is traced to `file:line` in the working tree.

**Headline:** the `unsafe` surface is small (89 production sites, 60 of them in the
out-of-workspace eBPF program) and I found **no unsoundness**. The panic surface is
genuinely well defended — the crate-level clippy deny-set plus a disciplined
`.get()`/`checked_*` style closes the classic wire-parser panics. **I found no reachable
panic on network input.** The real output of this pass is therefore three things:
(1) the prior-art justification file is materially stale and now under-covers the tree;
(2) the `panic-freedom` CI job is a *lint-attribute presence grep*, not a panic audit,
and the lint set it presumes has concrete blind spots; (3) six of the nine fuzz targets
drive code that is **not linked into the release binary**, while several live parsers
have no target at all.

---

## 0. Scope reality check — what is actually in the release binary

This reframes the brief's Priority-2 list and is load-bearing for every severity below.

| Crate | In `expressgateway` release binary? | Evidence |
| --- | --- | --- |
| `lb-h1` (`parse.rs`, `chunked.rs`) | **NO** | Not a dependency of any crate. Root `Cargo.toml:165` `[dev-dependencies]` + `fuzz/Cargo.toml` only. |
| `lb-h2` (`frame.rs`, `hpack.rs`) | **linked, never called** | `crates/lb-l7/Cargo.toml:42` — but the only uses are two constants (`crates/lb-l7/src/h2_security.rs:45,46,51,52,90,94`). Real H2 parsing is `hyper`/`h2`. |
| `lb-h3-testcodec` | **NO** | `crates/lb-quic/Cargo.toml:68` is under `[dev-dependencies]`. |
| `lb-quic::public_header` | **YES** | unauthenticated UDP, every Mode-A datagram. |
| `lb-quic::h3_bridge` (incl. its own `ChunkDecoder`) | **YES** | H1↔H3 bridge; parses upstream H1 responses. |
| `lb-quic::{passthrough,router,raw_proxy,conn_actor}` | **YES** | unauthenticated UDP. |
| `lb-grpc::{frame,deadline}` | **YES** | `crates/lb-l7/Cargo.toml:60`, used by `grpc_proxy.rs`. |
| `lb-security::retry` | **YES** | `router.rs:163`, `passthrough.rs:591`. |
| `lb-io::ring::{accept_one,recv,send,splice}` | **NO** | only `ring::nop_roundtrip()` is called, at `lb-io/src/lib.rs:68` and `:239`, as a capability probe. Matches the module doc at `lb-io/src/lib.rs:1-3`. |

A panic in `lb-h1`/`lb-h2`/`lb-h3-testcodec` is **not** a remote DoS. It is a fuzz-target
crash and nothing more.

---

## 1. `unsafe` inventory — every site, with a verdict

**Totals (measured, excluding `unsafe` appearing only inside comments):**
96 `unsafe` tokens in code across 10 files · **89 outside `#[cfg(test)]`** · 7 test-only.
Zero `unsafe impl Send` / `unsafe impl Sync` anywhere in the tree (verified:
`rg -n 'unsafe impl' --type rust crates/` returns only the 5 `Pod` impls).

### 1.1 `crates/lb-io/src/ring.rs` (13 sites; 12 production, 1 test)

| Line | Site | What it does | Verdict |
| --- | --- | --- | --- |
| 29 | `push_sqe(&mut ring, &nop)` | NOP submit; references no caller memory | **SOUND** |
| 61 | `push_sqe(accept entry)` | kernel writes `addr_storage`/`addr_len` (stack locals) | **UNDER-JUSTIFIED** (see RT-UNSAFE-02) |
| 69 | `sockaddr_storage_to_socketaddr(&addr_storage, addr_len)` | typed view of ACCEPT output | **SOUND** |
| 85 | `push_sqe(recv entry)` | kernel writes caller's `&mut [u8]` | **UNDER-JUSTIFIED** (RT-UNSAFE-02) |
| 103 | `push_sqe(send entry)` | kernel reads caller's `&[u8]` | **UNDER-JUSTIFIED** (RT-UNSAFE-02) |
| 120 | `push_sqe(splice entry)` | fds only, no caller memory | **SOUND** |
| 133 | `unsafe fn push_sqe` (decl) | invariant documented at definition | **SOUND** |
| 136 | `sq.push(entry)` | forwarded from caller | **SOUND** |
| 174 | `unsafe fn sockaddr_storage_to_socketaddr` (decl) | invariant documented | **SOUND** |
| 179 | `&*storage.as_ptr()` | `MaybeUninit::<sockaddr_storage>::zeroed()` (line 45); `sockaddr_storage` is `repr(C)` all-integer, so the all-zero bit pattern is valid | **SOUND** |
| 191 | `from_ref(storage_ref).cast::<sockaddr_in>()` | after `AF_INET` tag check + `addr_len >= size_of::<sockaddr_in>()`; align 8 → 4 is a downgrade | **SOUND** |
| 206 | `.cast::<sockaddr_in6>()` | same, `AF_INET6` | **SOUND** |
| 262 | `libc::close(accepted_fd)` | `#[cfg(test)]` (mod starts at 222) | **SOUND (test)** |

Bounds arithmetic is right in the safe direction: `recv`/`send` use
`u32::try_from(buf.len()).unwrap_or(u32::MAX)` (lines 76, 95), which can only *shrink*
the length the kernel is given relative to the buffer — never grow it.

### 1.2 `crates/lb-io/src/sockopts.rs` + `lib.rs`

| Line | Site | Verdict |
| --- | --- | --- |
| `sockopts.rs:106` | `libc::listen(fd, backlog)` — no memory crosses | **SOUND** |
| `sockopts.rs:248` | `libc::setsockopt(fd, …, addr_of!(value), len)` — `len` from `size_of::<c_int>()`, `value` a stack local, kernel does not retain | **SOUND** |
| `lib.rs:297` | `libc::getsockopt` — inside `#[cfg(test)] mod tests` (starts at 175); the prior audit lists it as a production site | **SOUND (test)** |

### 1.3 `crates/lb-l4-xdp/src/loader.rs` — `unsafe impl Pod` (5 sites)

Layouts computed by hand. `aya::Pod` requires `Copy + 'static` and a type whose full
byte range is meaningful (padding bytes are read by `bpf_map_update_elem`, so implicit
padding = reading uninit memory + an ABI mismatch with the BPF side).

| Line | Type | Hand-computed layout | Padding | Verdict |
| --- | --- | --- | --- | --- |
| 88 | `FlowKey` | u32@0, u32@4, u16@8, u16@10, u8@12, `[u8;3]`@13 = 16, align 4 | none | **SOUND** |
| 132 | `BackendEntry` | u32@0, u32@4, u16@8, u16@10, `[u8;6]`@12, `[u8;6]`@18 = 24, align 4 | none | **SOUND** |
| 206 | `FlowKeyV6` | `[u8;16]`@0, `[u8;16]`@16, u16@32, u16@34, u8@36, `[u8;3]`@37 = 40, align 2 | none | **SOUND** |
| 248 | `BackendEntryV6` | u32@0, `[u8;16]`@4, u16@20, u16@22, `[u8;6]`@24, `[u8;6]`@30 = 36, align 4 | none | **SOUND** |
| **346** | **`BackendTable`** | u32@0, u32@4, `[BackendEntry;64]`@8 (1536 B), u32@1544, u32@1548, `[BackendEntry;64]`@1552 = **3088**, align 4 | none | **SOUND but UNDOCUMENTED — see RT-UNSAFE-04/05** |

All five are `#[repr(C)]`, contain only integers and `[u8;N]`, no `bool`/`char`/enum/
reference/`NonZero`. Sizes are pinned by `const _: () = assert!(...)` at
`loader.rs:314-317` and `:378`. `publish_backends_v4` (`loader.rs:711-743`) bounds
`count` before any map write (`> MAX_BACKENDS_PER_VIP` → `TooManyBackends`, returned
*before* the write) and fully initialises every entry via `BackendEntry::new`, which
zeroes `pad`.

### 1.4 `crates/lb-l4-xdp/src/netlink_xdp.rs` (6 sites) — all **SOUND**

254 `if_nametoindex` (valid `CString`, `0` handled) · 269 `libc::close` in an RAII
`OwnedFd` guard · 279 `socket(AF_NETLINK, …)` (`fd < 0` checked, 286) · 304
`mem::zeroed::<sockaddr_nl>()` (all-integer `repr(C)`) · 309 `sendto` (`sent < 0`
checked, 319) · 326 `recv` into a `vec![0u8; 32*1024]`, `n < 0` checked (327) and
`reply.truncate(usize::try_from(n).unwrap_or(0))` — `recv` cannot exceed the buffer, so
the truncate is in-range.

The netlink *response* parser (`parse_getlink_response`, `RtattrIter`, `align`,
`read_u16/u32/i32`, lines 51-238) is the cleanest code I read this pass: `checked_add`
on every offset, `.get()` on every slice, and an explicit non-advancing-position guard
(`:232`) against an infinite loop. No finding.

### 1.5 `crates/lb-l4-xdp/src/bpffs.rs` (2 sites) — **SOUND**

33 `mem::zeroed::<libc::statfs>()` (all-integer `repr(C)`) · 34 `libc::statfs(c_path.as_ptr(), &mut buf)`
with `rc != 0` checked at 35. The `CString::new` interior-NUL case is a hard error (22-28).

### 1.6 `crates/lb-l4-xdp/ebpf/src/main.rs` (60 sites) — **SOUND**, verifier-backed

Out-of-workspace `no_std` BPF object; the in-kernel verifier is the backstop and
`scripts/verify-xdp.sh` diffs verifier logs across 5.15/6.1/6.6.

Categories (line ranges): `#[unsafe(link_section)]`/`#[unsafe(no_mangle)]` (28-29) ·
`unsafe fn ptr_at`/`ptr_at_mut` (372, 388) + ~24 call sites · ~26
`core::ptr::read_unaligned` packed-header field reads · 6 aya map accessors
(`BACKENDS_V4.get`, `CONNTRACK.get`, `L7_PORTS.get`, per-CPU `get_ptr`/`get_ptr_mut`).

Two things I checked specifically because they are the usual failure modes, and both are
**correct**:

- `ptr_at` (372-383) uses `start.checked_add(offset)?.checked_add(len)?` then `needed > end`.
  This is *better* than the common aya idiom — it is explicitly hardened against the
  CVE-2022-23222-class bounds-check elision (comment at 361-370).
- Packet-offset arithmetic cannot wrap: `ihl_words` is masked to 0x0F and floored at 5
  (`:497-501`) so `ip_hdr_len ∈ [20,60]`; the IPv6 ext-header walk is bounded to 2
  iterations with `off += (usize::from(len) + 1) * 8`, `len ≤ 255` ⇒ `off ≤ l3+40+4096`
  (`:745-763`). Every subsequent read goes back through `ptr_at`, which bounds-checks.

### 1.7 `crates/lb-soak/src/gateway.rs` (5 sites) — test infrastructure

145 `unsafe extern "C" { fn kill(pid: i32, sig: i32) -> i32; }` + 149 the call
(`send_sigterm`); 170/175/179 `std::env::{set_var,remove_var}` inside `#[cfg(test)]`
(mod starts at 155). `lb-soak` links no product crates and is not in the release binary.
Two notes, neither a finding: `kill(pid as i32, …)` would target a *process group* if the
cast went negative, but `Child::id()` is bounded by `pid_max` (≤ 2^22); and the
`env::set_var` calls are the Rust-2024 `unsafe` env API used from a test that `cargo test`
may run alongside other threads.

### 1.8 Test-only `unsafe` (2 sites) — **SOUND**

`crates/lb-l4-xdp/tests/pod_padding.rs:25` `transmute_copy` (guarded by an
`assert_eq!(N, size_of::<T>())` at 21) · `crates/lb-io/tests/miri_ring.rs:55`
`slice::from_raw_parts_mut` over a live stack `buf`.

---

## 2. Findings

### RT-UNSAFE-01 [MEDIUM] QUIC Retry-token expiry is defeated by a process restart (CWE-613 / CWE-294)

- CVSS: `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:N/I:L/A:N` (4.7) — impact is a widened
  address-validation replay window, not data.
- Location: `crates/lb-security/src/retry.rs:96` (`origin: Instant::now()`),
  `:128-130` (mint), `:193-197` (verify). Reached from
  `crates/lb-quic/src/router.rs:163` and `crates/lb-quic/src/passthrough.rs:591`.
- Class: token freshness bound anchored to volatile state while the key is persistent.
- **This is not a panic/`unsafe` finding.** It surfaced while auditing `Instant + Duration`
  overflow. Flagging for the lead to dedup with `rfc-quic` / `rt-dos`.

**Root cause.** `mint_at` stores `issued_ms = now.saturating_duration_since(self.origin).as_millis()`
— *milliseconds since this signer object was constructed*. `verify` reconstructs
`issued_at = self.origin + Duration::from_millis(issued_ms)` and then
`age = now.saturating_duration_since(issued_at)`. Within one process this is correct.
But the HMAC secret is **persisted to disk** and reloaded on start
(`crates/lb-quic/src/listener.rs:363-395` `load_or_generate_retry_secret`, and the
identical function at `crates/lb-quic/src/passthrough.rs:1016-1050`), while `origin` is
re-seeded to `Instant::now()` at every construction. So after a restart the same key
verifies old tokens against a *new* epoch.

**Concrete sequence.** Gateway has been up 1 hour. A client is issued a Retry token; it
carries `issued_ms ≈ 3_600_000`. Operator restarts (deploy, OOM, crash-loop). At new-process
uptime 5 s, that token is replayed: `issued_at = origin_new + 3600 s`, i.e. ~1 hour in the
*future*; `saturating_duration_since` clamps to `0`; `age = 0 < max_age` ⇒ **accepted**.
The token stays accepted until the new process's uptime passes `3600 s + max_age`.
`DEFAULT_RETRY_MAX_AGE` is **10 seconds** (`retry.rs:26`), so the intended window is
widened by the previous process's entire uptime.

**Impact.** Retry is QUIC's anti-spoofing/anti-amplification control. The peer binding
still holds (`token_peer != peer` ⇒ `PeerMismatch`, `retry.rs:189`), so this is not a
free bypass — but it turns a 10 s window into an hours-long one for any address that was
ever validated. An off-path attacker who observed (or was issued) a token for address A
can, after a restart, spoof Initials from A and skip the Retry round-trip, getting the
server to allocate connection state and emit handshake bytes toward A.

**Would a test catch it?** No. `retry.rs:283-290` (`with_max_age(1s)` + `t0 + 1s`) only
exercises expiry *within one signer instance*. There is no cross-restart / cross-instance
test, and no fuzz target for `verify`.

### RT-UNSAFE-02 [LOW — latent, unreachable today] io_uring SQE may outlive the caller's buffer on the `submit_and_wait` error path

- CVSS: n/a (no reachable path). Would be `AV:L/AC:H` if wired up.
- Location: `crates/lb-io/src/ring.rs:61+63`, `:85+87`, `:103+105` (and `:120+122`).
- Class: CWE-416-adjacent — kernel-side use of memory the Rust side has released.

```rust
unsafe { push_sqe(&mut ring, &entry)? };   // SQE now queued
ring.submit_and_wait(1)?;                  // <-- `?` returns EARLY on error
```

The module doc (`ring.rs:1-3`) and the prior justification both rest on *"the caller's
stack storage outlives `submit_and_wait`"*. That holds on the success path. On the
**error** path it does not: `io_uring_enter` can return `-EINTR` after the SQE has already
been consumed by the kernel (this binary installs signal handlers — SIGHUP hot reload,
`crates/lb/src/main.rs`), and `?` then returns immediately. `ring` is dropped (unmapping
SQ/CQ and closing the ring fd) while a `IORING_OP_RECV` may still be in flight holding a
pointer to the *caller's* `&mut [u8]`, which the caller is now free to reuse or drop.
io_uring teardown on `close()` is asynchronous (`io_ring_exit_work`), so this is a real
race, not a theoretical one.

**Why it is LOW, not HIGH:** `recv`, `send`, `accept_one` and `splice` have **no production
caller** (§0). Only `nop_roundtrip()` runs, and a NOP references no caller memory — that
one is sound. The finding is that the *justification* is wrong, so the moment anyone wires
these into the datapath the bug is live. The fix is to reap or cancel before returning
(or to hold the buffer until the CQE is observed) rather than `?`.

### RT-UNSAFE-03 [LOW — latent] `accept_one` leaks the accepted fd on the address-decode error path

- Location: `crates/lb-io/src/ring.rs:65-69`.

`let fd = check_cqe(&cqe)?;` succeeds, then
`let addr = unsafe { sockaddr_storage_to_socketaddr(&addr_storage, addr_len)? };` can
return `Err` (unexpected family, or `addr_len` shorter than the family's `sockaddr`) —
and `fd` is dropped without `close(2)`. Note the prior audit table cites
`lb-io/src/ring.rs:345 · libc::close(accepted_fd) · "close on cleanup path"`: **that line
does not exist** — the file is 321 lines and the only `libc::close` is at 262, inside
`#[cfg(test)]`. Dead code today, so no live fd-exhaustion.

### RT-UNSAFE-04 [INFO] `audit/unsafe-justifications.md` is materially stale and under-covers the tree

Not a vulnerability; it is the reason RT-UNSAFE-02/03/05 were not caught earlier. Nothing
in CI keeps it in sync (the `panic-freedom` job does not touch it; `doc-lint.sh` checks
stale patterns and Verified-Fixed SHAs, not this inventory).

| Doc claim | Reality |
| --- | --- |
| "73 occurrences across two crates" | 96 in code across **three** crates (`lb-io`, `lb-l4-xdp`, `lb-soak`) |
| lb-l4-xdp = 57 (4 `Pod` + 53 ebpf) | 73 (5 `Pod` + 60 ebpf + 6 netlink + 2 bpffs) |
| 4 `unsafe impl Pod` sites | **5** — `unsafe impl Pod for BackendTable` at `loader.rs:346` is absent |
| `netlink_xdp.rs` | **entirely absent** (6 FFI sites) |
| `bpffs.rs` | **entirely absent** (2 FFI sites) |
| `lb-soak/src/gateway.rs` | **entirely absent** (5 sites) |
| "All other crates are `#![forbid(unsafe_code)]` or contain zero `unsafe`" | **False.** `rg 'forbid\(unsafe_code\)'` returns **zero hits** workspace-wide. The only `unsafe_code` attribute in the tree is `#![allow(unsafe_code)]` at `netlink_xdp.rs:29`. The *second* clause happens to be true, which is why the conclusion is not wrong — but the stated mechanism does not exist. |
| lb-io line numbers (50, 99, 110, 133, 157, 182, 200, 203, 255, 260, 273, 289, **345**) | Actual: 29, 61, 69, 85, 103, 120, 133, 136, 174, 179, 191, 206, 262. **Every one is wrong**, and 345 is past EOF. |
| `lb-io/src/lib.rs:343` listed as production `getsockopt` | now `:297`, and it is inside `#[cfg(test)]` |

Note the release-profile comment in the workspace `Cargo.toml` inherits the same drift:
*"the 17 unsafe blocks in lb-io::ring + the 4 unsafe-impl-Pod sites"* — the counts are 12
(production) and 5.

### RT-UNSAFE-05 [INFO] `pod_padding.rs` covers 4 of the 5 `Pod` types

`crates/lb-l4-xdp/tests/pod_padding.rs` proves pad-zeroing for `FlowKey`, `FlowKeyV6`,
`BackendEntry`, `BackendEntryV6` and re-asserts their sizes. **`BackendTable` (3088 B, the
value type of the atomic per-VIP publication map) has neither.** Its layout is
padding-free today (§1.3) and `loader.rs:378` const-asserts the size, so this is not a
live defect — but the class of bug the test file exists to catch (a `pad` field dropped,
a field width changed, a `MaybeUninit` constructor introduced) is unguarded for the
largest and newest of the five.

### RT-CONF-01 [LOW–MEDIUM · needs-review] Live chunked decoder accepts non-RFC chunk sizes; the hardened sibling implementation is dead code (CWE-444)

- CVSS: `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:L/I:L/A:N` (5.8) — rated on a topology-dependent
  response-desync primitive, not a demonstrated exploit. **Flagging for `rt-smuggle` /
  `rfc-h1` to own the impact call; I am reporting the parser deviation and the
  dead-vs-live split.**
- Location: `crates/lb-quic/src/h3_bridge.rs:501-506` (`ChunkDecoder::feed`, the `None`
  chunk-size arm). Sole production call site: `h3_bridge.rs:737`, inside
  `stream_h1_response` — the **response** direction (H1 upstream → H3 client).
- Class: RFC 9112 §7.1.1 `chunk-size = 1*HEXDIG` deviation / parser laxity.

```rust
let hex_end = line.iter().position(|&b| b == b';').unwrap_or(line.len());
let hex = std::str::from_utf8(line.get(..hex_end).unwrap_or(line))
    .map_err(|_| RespAbort::ChunkedDecode)?
    .trim();                                                   // (a)
if hex.is_empty() { return Err(RespAbort::ChunkedDecode); }
let size = usize::from_str_radix(hex, 16).map_err(|_| RespAbort::ChunkedDecode)?;  // (b)
```

Two accepted-but-illegal forms:

**(a) `.trim()`** strips `char::is_whitespace` from both ends, so `"  5\r\n"`, `"5 \r\n"`
and even `"\u{a0}5\r\n"` (U+00A0 NBSP is `White_Space`, and the bytes reach `trim` after
`from_utf8`) all decode as chunk size 5. RFC 9112 §7.1.1 permits BWS only *between* the
size and a following `";"` — never leading, and never trailing without a `";"`.

**(b) `usize::from_str_radix` accepts a leading `+`.** Verified in the pinned toolchain's
own std source, `~/.rustup/toolchains/1.88-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/num/mod.rs:1536-1541`
— `[b'+', rest @ ..] => (true, rest)` applies to unsigned types too. So
`usize::from_str_radix("+5", 16) == Ok(5)`. Combined with (a), `" +5 \r\n"` is a valid
chunk header to this decoder. `+` is not a `HEXDIG`.

**Why this matters more than a conformance nit.** The workspace already contains a
*correct* chunk-size lexer — `lb-h1::parse_chunk_size_hex` (`crates/lb-h1/src/chunked.rs:290-309`),
which rejects empty input, >16 digits, and any non-`HEXDIG` byte, and uses `checked_shl`.
Its own doc comment (`chunked.rs:119-122`) names the exact CVEs it was hardened against
(nginx CVE-2013-2028, hyper GHSA-5h46-h7hh-c6x9, the HAProxy `h1_append_chunk_size`
stack overflow). **That hardened lexer is in a crate no production code links (§0), and it
is the one the `h1_chunked` fuzz target drives.** The lexer that actually runs in the
binary is this second, laxer implementation, and it has no fuzz target. The audit trail
therefore records the chunk-size-lexer class as hardened while the shipped decoder is not.

**Impact, stated honestly.** This is the response direction, so the byte author is the
upstream (semi-trusted) and the receiver is the gateway — no party asymmetry on its own,
hence not a straight smuggling finding. It becomes a desync primitive only where a third
hop parses the same octets: a cache/CDN or a second proxy between gateway and origin that
rejects `" +5"` while we accept it (or the reverse) splits the response stream ⇒ response-queue
or cache poisoning. It is also a plain interop/conformance defect against nginx, HAProxy
and hyper, all of which reject both forms. No memory-safety consequence: `MAX_CHUNK_SIZE_LINE`
(`h3_bridge.rs:430`) caps the line at 256 bytes and `from_str_radix::<usize>` errors on
overflow rather than wrapping.

**Would a test catch it?** No. `h3_bridge.rs:2552-2660` has 12 `ChunkDecoder` unit tests;
none feeds a whitespace- or sign-prefixed chunk size. The one test that probes the size
line is the `MAX_CHUNK_SIZE_LINE` overflow case at `:2583`.

### RT-PANIC-01 [INFO] `GrpcDeadline::parse_timeout` — `str::split_at` on a non-char-boundary

- Location: `crates/lb-grpc/src/deadline.rs:19`
  `let (digits_str, unit_char) = value.split_at(value.len() - 1);`
- `str::split_at(mid)` panics if `mid` is not a UTF-8 char boundary. For `"1é"`
  (`31 C3 A9`, len 3) `value.len()-1 == 2` lands inside the 2-byte `é` ⇒ panic.
  The empty case *is* guarded (`:15-17`), so the subtraction itself cannot underflow.

**Not reachable in production.** The only production caller is
`crates/lb-l7/src/grpc_proxy.rs:259`, which obtains the `&str` from `hv.to_str()` at
`:259`/`:255-258`. Verified in the pinned `http-1.4.1`
(`~/.cargo/registry/.../http-1.4.1/src/header/value.rs:240-250` + `is_visible_ascii` at
`:552-554`): `to_str()` returns `Err` unless **every** byte is in `32..=126` or `\t`, so
the value is pure ASCII and every byte index is a char boundary. Filed as INFO because
`parse_timeout` is a `pub fn` whose doc contract says it returns `Err` on anything not
matching the grammar — it panics instead on one input class — and because there is no
fuzz target for it.

---

## 3. Panic-reachability triage — coverage ledger

Method: (a) grep every panic-capable construct across the 14 production crates;
(b) discard `#[cfg(test)]` and `#[allow]`-covered regions; (c) hand-trace the survivors.

| Construct | Hits swept | Production, wire-reachable | Verdict |
| --- | --- | --- | --- |
| `.unwrap()` / `.expect()` / `panic!` / `todo!` / `unimplemented!` / `unreachable!` | — | 0 | Clippy-denied at every crate root; **every** `#[allow(clippy::unwrap_used\|expect_used\|panic)]` in the tree sits on a `#[cfg(test)] mod tests` or a `tests/*.rs` file. Verified individually for `lb-core/src/authority.rs:73`, `lb-l7/src/authority.rs:47`, `lb-l7/src/{h1_proxy.rs:2450,h2_proxy.rs:2706,grpc_proxy.rs:478,ws_proxy.rs:409,upstream.rs:197,trace_ctx.rs:194}`, `lb-io/src/{http2_pool.rs:337,pool.rs:382,dns.rs:321}`, `lb-l4-xdp/src/{nic_compat.rs:310,loader.rs:1210,1237}`. |
| `[i]` / `[a..b]` indexing | — | 0 | `clippy::indexing_slicing` denied at every crate root. The only two `#[allow(clippy::indexing_slicing)]` in production source are `crates/lb/src/main.rs:4238` and `:4256` — both past `#[cfg(test)] mod tests` (starts at `:3311`). |
| `assert!` / `assert_eq!` at runtime | 10 | **0** | All 5 hits outside tests are `const _: () = assert!(...)` (compile-time) at `loader.rs:314-317,378`. |
| `debug_assert!` | 2 | 0 (non-load-bearing) | `lb-security/src/conn_gate.rs:173` (`prev > 0` after `fetch_sub`; atomics wrap, they never panic, and the permit/decrement invariant holds — `ConnPermit` is constructed only in `admit`) and `lb-security/src/handshake.rs:42`. |
| `%` / `/` by a computed divisor | 2 | 0 | `lb-balancer/src/round_robin.rs:24` and `session_affinity.rs:36` are both preceded by an `is_empty()` → `NoBackends` guard (`:21-23`, `:31-33`). Audited **all 12** balancers: every `pick`/`pick_with_key` guards empty, and `weighted_random.rs:27` / `weighted_round_robin.rs:55` additionally guard `total_weight == 0`. |
| `.copy_from_slice()` | 8 | 0 | `lb-security/src/retry.rs:177,231,235,239` all copy into a fixed array from a `.get()`-derived slice of the exact matching width; `lb-l7/src/trace_ctx.rs:91,92` and `lb-quic/src/passthrough.rs:844` write into fixed arrays; `listener.rs:377`/`passthrough.rs:1030` copy `bytes.get(..RETRY_SECRET_LEN).unwrap_or(&[0;N])` after an explicit length check. |
| `.split_at()` | 1 | 0 | RT-PANIC-01. |
| `.split_to()` / `.split_off()` / `.drain(..n)` | 14 | 0 | `h3_bridge.rs:473,485,511,543,560` — `nl` always comes from `windows(2).position(...)` so `nl+2 ≤ len`; `:611` `head.split_off(sep+4)` where `sep` comes from `find_header_sep` = `windows(4).position(...)` (`:368-370`) so `sep+4 ≤ len`. `raw_proxy.rs:920` and `main.rs:3719` use `.min(len)`. `chunked.rs` (dead code) uses the same `windows`-derived indices. |
| `Vec::insert(i,..)` / `Vec::remove(i)` | 6 | 0 | `h2_proxy.rs:2195-2205` and `:2533-2540` insert at 0,1,2 then 3 — len ≥ 3 by then. `h2_to_h1.rs:77` `remove(idx)` where `idx` comes from `.iter().enumerate().find(..)` on the same `Vec`. |
| `chunks(n)` / `chunks_exact(n)` | 5 | 0 | all constant non-zero `n`. |
| `Instant + Duration` (panics on overflow) | 3 production | 0 | `retry.rs:193` — operand is MAC-authenticated and bounded by process uptime (but see RT-UNSAFE-01 for the *semantic* bug); `idle_send.rs:57,81` — `lp_ms` is a millis-since-epoch progress counter. |
| Unguarded `+`/`*` used in a limit comparison | 4 | 0 | `h2/frame.rs:138` (`pad_len ≤ 255`), `:305` (`i` bounded by payload), `h2/hpack.rs:111`, `h3_bridge.rs:530` (both bounded by allocated memory). |
| `len() - n` subtraction | 15 | 0 | `retry.rs:162` `token.len() - MAC_LEN` is preceded by `token.len() < 1+8+1+4+2+1+MAC_LEN` → `Truncated` (`:151`), so `len ≥ 49 > 32`. `h2/frame.rs:144` is guarded by `:138`. `hpack.rs:148` `index - static_len - 1` is inside the `else` of `index <= static_len`. `passthrough.rs:1139` is `#[cfg(test)]`. |

**Wire-parser deep reads (line-by-line, whole function):**
`lb-quic/src/public_header.rs` (613 L) · `lb-quic/src/h3_bridge.rs` ChunkDecoder +
`stream_h1_response` (429-700) · `lb-grpc/src/frame.rs` (92 L) ·
`lb-grpc/src/deadline.rs` (72 L) · `lb-security/src/retry.rs` verify/decode_peer
(144-243) · `lb-l4-xdp/src/netlink_xdp.rs` (345 L) ·
`lb-observability/src/tracing_propagation.rs` `parse_traceparent` (95-140) ·
`lb-h2/src/{frame.rs,hpack.rs}` (dead code, read anyway) ·
`lb-h1/src/chunked.rs` (dead code) · `lb-l4-xdp/ebpf/src/main.rs` parse paths.

`public_header.rs` deserves the callout: it is the only hand-written parser on
unauthenticated UDP and it is **exemplary** — `.first()`/`.get()` on every read,
`checked_add` on every offset, `saturating_sub` in the error constructors, and the
`decode_varint` shift is structurally bounded to `1usize << (first >> 6) ∈ {1,2,4,8}`.
The prior S38 verdict on it stands.

---

## 4. The `panic-freedom` CI gate — what it actually checks, and what it misses

**This is the highest-value section per the brief.**

`.github/workflows/ci.yml:75-94`:

```bash
for lib in crates/*/src/lib.rs; do
  if ! grep -Pzoq '#!\[deny\([^)]*clippy::unwrap_used' "$lib"; then MISSING="$MISSING\n  $lib"; fi
done
```

It does not grep for a single `unwrap()`, `panic!`, or `[i]`. It asserts that a *lint
attribute is present in a file*. Real enforcement is entirely the separate `clippy` job
(`ci.yml:63-73`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`),
which is sound and thorough. So the gap analysis is really "what does the deny-set not
cover, and where does the presence-check fail to protect it".

### G1 — The file glob is `crates/*/src/lib.rs` only

Binary crate roots are separate crates; a `#![deny]` in `lib.rs` does **not** apply to them.
Not checked: `crates/lb/src/main.rs` (the production binary — it *does* carry the full
deny-set at `:2-11`, but by luck, not by gate), `crates/lb-soak/src/bin/eg-soak.rs`
(`#![allow(clippy::expect_used, clippy::unwrap_used)]` at `:8`, no deny),
`crates/lb-soak/src/bin/eg-bench.rs` (same at `:9`). A future `src/bin/` entry point or a
new binary crate is invisible to this gate.

### G2 — It checks for exactly one lint out of seven

The product deny-set is `unwrap_used, expect_used, panic, indexing_slicing, todo,
unimplemented, unreachable`. The gate only requires `clippy::unwrap_used`. A crate can
drop the other six and stay green — and one already has: `crates/lb-soak/src/lib.rs:9`
denies only `unwrap_used, expect_used, panic` (no `indexing_slicing`, no
`todo`/`unimplemented`/`unreachable`) and the gate passes.

### G3 — A later `#![allow]` / per-item `#[allow]` is invisible

The regex only proves the `deny` attribute *exists*. `#![allow(clippy::unwrap_used)]`
three lines below it, or `#[allow(...)]` on a production module, silently re-opens the
hole. Today there is no such production bypass (every `#[allow]` I checked is on a test
module — enumerated in §3), but nothing detects one landing.

### G4 — The deny-set itself does not cover the panics a wire parser actually reaches

This is the substantive gap. **Not denied anywhere in the workspace:**

1. **`clippy::arithmetic_side_effects`** — integer overflow/underflow and division by a
   computed zero are entirely unlinted. Combined with `overflow-checks` being off in
   `[profile.release]`, a wire-derived `a - b` **silently wraps to ≈`usize::MAX`** in
   production while panicking in debug/test. A wrapped value used as a limit or a
   capacity is a security bug that no gate and no green test run will show. I found no
   live instance (§3), but the class is completely unguarded.
2. **Panicking *library* calls — no clippy lint exists for any of these.** `indexing_slicing`
   catches `a[i]`; it does not catch `copy_from_slice` (length mismatch), `split_at` /
   `split_to` / `split_off` / `Buf::advance` (index > len), `Vec::{insert,remove,drain,swap}`,
   `chunks(0)`, `Bytes::slice`, `Buf::get_u32` (insufficient remaining),
   `Duration::from_secs_f64` (NaN/negative/overflow), `Instant + Duration` (overflow),
   `Vec::with_capacity` (capacity overflow ⇒ `handle_alloc_error` ⇒ abort), or
   `sort_by` with an inconsistent comparator (panics since Rust 1.81). These are the
   *actual* panic surface of this codebase — 211 such call sites exist across the
   workspace, and all 14 production-reachable ones had to be hand-traced (§3) because
   nothing automated can see them.
3. **`assert!` / `assert_eq!`** — there is no clippy restriction lint for them, so
   `clippy::panic` does not cover them. Zero exist in production code today; nothing
   prevents one landing.
4. **`clippy::string_slice`** — `&s[a..b]` on a `str` panics on a non-char-boundary
   (RT-PANIC-01 is that shape, via `split_at`). Not enabled.
5. **Panics inside dependencies driven by our input** — hyper, quiche, `h2`,
   tokio-tungstenite. Out of reach of any lint.

### G5 — `debug_assert!` is a release no-op and the gate cannot tell load-bearing from not

Both instances (`conn_gate.rs:173`, `handshake.rs:42`) are non-load-bearing (§3), but
that had to be established by hand.

### G6 — Impact of any escape is maximal

`[profile.release] panic = "abort"` (workspace `Cargo.toml`), and
`init_panic_hook` (`crates/lb/src/main.rs:71-102`) explicitly `tracing::error!`s, sleeps
50 ms, then `std::process::abort()`. So **one reachable panic anywhere = whole-process
abort = every in-flight connection on that instance dropped.** The `panic_total` counter
(`:107-118`) records it, which is good post-mortem hygiene — but it is a tombstone, not a
control.

**Recommended gate hardening (for the lead's second pass, not applied here):**
add `clippy::arithmetic_side_effects` (or at minimum on the parser crates) to the
deny-set; extend the CI loop to `crates/*/src/lib.rs`, `crates/*/src/main.rs` and
`crates/*/src/bin/*.rs`; assert the **full** seven-lint set rather than just
`unwrap_used`; and add a grep for `#[allow(clippy::(unwrap_used|expect_used|panic|indexing_slicing))]`
outside `#[cfg(test)]`/`tests/`.

---

## 5. Fuzz-coverage gap

`fuzz/fuzz_targets/` holds 9 targets. Cross-referenced against §0:

| Target | Drives | In the release binary? |
| --- | --- | --- |
| `h1_parser` | `lb_h1::parse_headers_with_limit` | **NO** |
| `h1_request_line` | `lb_h1::parse_request_line` | **NO** |
| `h1_chunked` | `lb_h1::ChunkedDecoder` (the **hardened** lexer) | **NO** — see RT-CONF-01 |
| `h2_frame` | `lb_h2::decode_frame` | **NO** (linked, never called) |
| `h2_hpack` | `lb_h2::HpackDecoder` | **NO** (linked, never called) |
| `h3_frame` | `lb_h3_testcodec::decode_frame` | **NO** (test-only crate by design) |
| `quic_initial` | `quiche::Header::from_slice` | yes (dependency) |
| `quic_public_header` | `lb_quic::public_header::parse_public_header` | **YES** |
| `tls_client_hello` | `rustls::server::Acceptor` | yes (dependency) |

**Six of nine targets fuzz code that is not in the shipped binary.** Only one target
covers a first-party production parser.

Production parsers with **no** fuzz target:

- **`lb_quic::h3_bridge::ChunkDecoder::feed` / `parse_trailer_section`** (`h3_bridge.rs:454-563`)
  — the *live* chunked decoder, on the H3-egress path, decoding upstream H1 responses.
  The `h1_chunked` target fuzzes the other, dead one in `lb-h1`. **There are two chunked
  decoders in this repo, the fuzzer points at the dead one, and the live one is the laxer
  of the two — that is RT-CONF-01.** Highest-value gap, and the concrete proof that this
  mismatch is not academic.
- `lb_quic::h3_bridge::stream_h1_response` head parse + `parse_status_line` (`:368-378`, `:580-620`).
- `lb_grpc::frame::decode_grpc_frame` and `lb_grpc::deadline::parse_timeout` (RT-PANIC-01).
- `lb_security::retry::RetryTokenSigner::verify` — runs on unauthenticated UDP
  (`router.rs:163`, `passthrough.rs:591`) and parses an attacker-supplied token buffer.
- `lb_core::authority::validate` — the shared `:authority`/Host predicate.
- `lb_observability::tracing_propagation::parse_traceparent` / `parse_tracestate` —
  attacker-controlled request headers.
- `lb_l4_xdp::netlink_xdp::parse_getlink_response` — kernel-supplied input.

---

## 6. Coverage statement (so the negatives are auditable)

- `unsafe`: **100%** — all 96 in-code sites read and adjudicated (table §1). No UNSOUND.
- `unsafe impl Send` / `Sync`: **zero in the tree**, verified.
- Panic constructs: swept all 14 production crates for 15 construct classes; every
  production-reachable hit hand-traced (§3 ledger). Roughly 260 raw hits triaged; 0
  reachable panics found.
- Not covered by this pass (other agents' lanes): protocol-semantic correctness,
  smuggling, resource exhaustion / unbounded buffering, TLS/cert handling, config
  validation, the eBPF program's *routing logic* (as opposed to its memory safety).
