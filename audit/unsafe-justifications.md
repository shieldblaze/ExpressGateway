# Audit — `unsafe` Justifications (Round 7 Gate 5)

This file enumerates every `unsafe` site in the workspace (excluding
`tests/` and `*_test.rs`) and supplies a one-line rationale plus a
pointer to the in-source `SAFETY:` comment. Generated from
`grep -rn 'unsafe' crates/`.

**Totals:** 73 occurrences across two crates.

| Crate        | Count | Notes                                                  |
| ------------ | ----- | ------------------------------------------------------ |
| `lb-io`      | 16    | libc + io_uring FFI shims                              |
| `lb-l4-xdp`  | 57    | XDP/eBPF program (no_std) + `Pod` impls for bpf maps   |

All other crates (`lb-l7`, `lb-quic`, `lb-balancer`, `lb-security`,
`lb-observability`, `lb-config`, …) are `#![forbid(unsafe_code)]` or
contain zero `unsafe` blocks — confirmed by the grep above.

---

## Crate: `lb-io` (libc + io_uring shims)

All sites are FFI calls into `libc` / `io_uring` whose safety invariants
are documented in-source. The pattern is consistent: stack-local
arguments outlive the syscall, the kernel does not retain pointers past
return, and lengths are bounded.

| File:Line                            | Site (abbrev)                          | Rationale                                                                                     |
| ------------------------------------ | -------------------------------------- | --------------------------------------------------------------------------------------------- |
| `lb-io/src/sockopts.rs:124`          | `libc::listen(fd, backlog)`            | FFI; `fd` is a valid socket FD owned by caller. No memory crossed.                            |
| `lb-io/src/sockopts.rs:292`          | `libc::setsockopt(fd, …)`              | FFI; `value` is a stack local, `len` matches `int`/`socklen_t` shape. See SAFETY ~L283.       |
| `lb-io/src/lib.rs:343`               | `libc::getsockopt(fd, …)`              | FFI; out-pointer + `len` are stack locals matching kernel expectations. See SAFETY ~L335.     |
| `lb-io/src/ring.rs:50`               | `push_sqe(&mut ring, &nop)`            | Bootstrap NOP submit; ring just constructed (8 entries), one push cannot overflow.            |
| `lb-io/src/ring.rs:99`               | `push_sqe(accept entry)`               | Pointers live on caller stack; `submit_and_wait` blocks until kernel done. See SAFETY ~L92.   |
| `lb-io/src/ring.rs:110`              | `sockaddr_storage_to_socketaddr(…)`    | Caller asserts `addr_len` came from a successful ACCEPT CQE.                                  |
| `lb-io/src/ring.rs:133`              | `push_sqe(recv entry)`                 | `len_u32` bounded by slice length; buffer outlives sync wait.                                 |
| `lb-io/src/ring.rs:157`              | `push_sqe(send entry)`                 | Buffer is a borrowed slice that outlives the synchronous `submit_and_wait`.                   |
| `lb-io/src/ring.rs:182`              | `push_sqe(connect entry)`              | `addr` and `len` are caller-owned stack values held for the sync call.                        |
| `lb-io/src/ring.rs:200`              | `unsafe fn push_sqe`                   | Function-level unsafety forwarded to callers; documented invariant at definition.             |
| `lb-io/src/ring.rs:203`              | `sq.push(entry)`                       | Forwarded from caller; entry's referenced buffers outlive the call.                           |
| `lb-io/src/ring.rs:255`              | `unsafe fn sockaddr_storage_to…`       | Caller must guarantee `addr_len` matches a successful ACCEPT family-tagged storage.           |
| `lb-io/src/ring.rs:260`              | `&*storage.as_ptr()`                   | MaybeUninit guaranteed init by ACCEPT completion. See SAFETY ~L258.                           |
| `lb-io/src/ring.rs:273`              | `cast::<sockaddr_in>()`                | After AF_INET tag check; `repr(C)` layout compatible.                                         |
| `lb-io/src/ring.rs:289`              | `cast::<sockaddr_in6>()`               | After AF_INET6 tag check; `repr(C)` layout compatible.                                        |
| `lb-io/src/ring.rs:345`              | `libc::close(accepted_fd)`             | FD just produced by ACCEPT; close on cleanup path.                                            |

---

## Crate: `lb-l4-xdp` (XDP program + loader)

### Loader (`crates/lb-l4-xdp/src/loader.rs`) — `Pod` trait impls

| Line | Site                              | Rationale                                                                                       |
| ---- | --------------------------------- | ----------------------------------------------------------------------------------------------- |
| 98   | `unsafe impl Pod for FlowKey`     | `#[repr(C)]` POD; no padding; no references. SAFETY comment at impl.                            |
| 151  | `unsafe impl Pod for BackendEntry`| `#[repr(C)]` POD; layout-stable; serialised into bpf map.                                       |
| 197  | `unsafe impl Pod for FlowKeyV6`   | `#[repr(C)]` POD; IPv6 variant of FlowKey, same invariants.                                     |
| 241  | `unsafe impl Pod for BackendEntryV6`| `#[repr(C)]` POD; IPv6 variant of BackendEntry, same invariants.                              |

The `Pod` trait (from `aya`/zerocopy) requires the type to be
zero-initialisable and free of indirections. These structs match.

### eBPF program (`crates/lb-l4-xdp/ebpf/src/main.rs`)

This crate is `no_std`, compiled to BPF bytecode, and the verifier
enforces every memory access at load time. All `unsafe` blocks are
required by the aya-bpf API surface.

Categorisation of the 53 occurrences in `ebpf/src/main.rs`:

1. **`#[unsafe(link_section = "license")]`, `#[unsafe(no_mangle)]`** (L45-46):
   required by aya-bpf for the LICENSE symbol and program entry point.

2. **`unsafe fn ptr_at`, `unsafe fn ptr_at_mut`** (L260, L272): bounds-checked
   packet pointer access; required because BPF packet pointers are raw and
   the verifier inspects the bound check in the caller.

3. **`ptr_at` / `ptr_at_mut` call sites** (L287, L363, L371, L396, L421, L432,
   L489, …): each call is followed by a bound check the BPF verifier reads;
   if the bound check fails the program is rejected.

4. **`core::ptr::read_unaligned`** on `Ipv4Hdr`, `Ipv6Hdr`, `TcpHdr`, `UdpHdr`
   fields (L398, L404-406, L423, L426, L434, L437, …): network headers are
   not naturally aligned on packet boundaries; `read_unaligned` is the only
   sound way to read them in BPF.

5. **`L7_PORTS.get(&dst_port)`, `CONNTRACK.get(&key)`, `CONNTRACK.insert(…)`,
   `BACKENDS.get(…)`, `BACKENDS_V6.get(…)`, `FLOWS.insert(…)`** (L448, L464, …):
   aya-bpf map accessors are `unsafe fn` because they return raw references
   to map storage; the verifier ensures key/value layouts match.

6. **Checksum recompute helpers** (`u16::from_be(unsafe { read_unaligned … })`,
   etc.): bulk reads of header fields needed for incremental L4 checksum
   updates after rewriting daddr/dport.

Every site has a `// SAFETY:` comment in source. The aya-bpf model
guarantees soundness via the in-kernel verifier — any unsoundness would
cause the program to be rejected at load time, which is exercised by
the verifier-log matrix gate (`scripts/verify-xdp.sh` × {5.15, 6.1, 6.6}).

---

## Audit conclusion

* No `unsafe` outside `lb-io` (FFI) and `lb-l4-xdp` (eBPF, by definition).
* Every site has an in-source `SAFETY:` comment.
* All `lb-io` sites are tightly scoped to a single libc/io_uring call.
* `lb-l4-xdp` BPF sites are validated by the kernel verifier on every
  load and additionally pinned via the verifier-log diff gate in CI.

**Status: PASS** — `unsafe` surface is minimal, documented, and gated.
