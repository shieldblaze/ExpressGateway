# Batched low-severity plan — SEC-2-08, SEC-2-11, SEC-2-12

Finding-refs:    SEC-2-08, SEC-2-11, SEC-2-12 (all low, status: Open)
Owner:           sec
Lead-approval: approved 2026-05-13 team-lead

Three findings, same owner, same severity, small patch surface.
Batched into one Round-4 PR.

---

## SEC-2-08 — TLS private-key file permissions not asserted at load

**Files touched**:
- `crates/lb/src/main.rs` lines 202-211 (`load_private_key`)
- `crates/lb/src/main.rs` lines 187-198 (`load_cert_chain` — same
  treatment for the cert chain file)
- `crates/lb/tests/key_perm.rs` (new)

**Joint boundary**: `main.rs` is code-owned per §D. **Sec hands code
a 6-line patch** plus the test; code applies in CODE-2-01 era.

**Approach**: in `load_private_key`, call
`std::fs::metadata(path)?.permissions().mode()`; if `mode & 0o077 !=
0` then (a) warn unconditionally, (b) if
`[runtime].strict_mode = true`, return `Err`. Same in
`load_cert_chain`. Add a small helper
`lb_security::posix::assert_owner_only(path)` that both call sites
use — sec owns the helper.

**Proof**: `cargo test -p lb --test key_perm`:
- `test_chmod_0644_warns_in_lax_mode` — `chmod 0644` tempfile,
  `strict_mode=false`, key loads, warning observed in test logger.
- `test_chmod_0644_errors_in_strict_mode` — same with
  `strict_mode=true`, `load_private_key` returns `Err`.
- `test_chmod_0600_passes` — regression guard for the happy path.

**Cross-ref**: `crates/lb-quic/src/listener.rs:326-337` asymmetry —
sec also asserts the same helper is used there for consistency.

---

## SEC-2-11 — XDP capability probe misses CAP_SYS_ADMIN fallback

**Files touched**:
- `crates/lb/src/xdp.rs` lines 39-55 (probe site)
- `crates/lb/tests/xdp_cap_probe.rs` (new)

**Joint boundary**: `crates/lb/src/xdp.rs` is **ebpf-owned** per §D
(EBPF-2-04, EBPF-2-06). Sec hands ebpf a 4-line patch describing the
exact predicate change. **The patch lives in ebpf's plan EBPF-2-04**
in Round 4; sec's plan here documents the security rationale and
provides the test.

**Approach**: change the probe predicate from `has(CAP_BPF) &&
has(CAP_NET_ADMIN)` to `(has(CAP_BPF) || has(CAP_SYS_ADMIN)) &&
(has(CAP_NET_ADMIN) || has(CAP_SYS_ADMIN))`. Document the kernel-
floor as "5.8 with CAP_BPF or 5.4–5.7 with CAP_SYS_ADMIN" in the
error message.

**Proof**: `cargo test -p lb --test xdp_cap_probe`:
- `test_probe_accepts_cap_bpf_and_cap_net_admin` — mocked caps.
- `test_probe_accepts_cap_sys_admin_alone` — pre-5.8 emulation.
- `test_probe_rejects_neither` — no caps → clear error message
  naming both kernel floors.

---

## SEC-2-12 — BPF ELF license / loader license-string

**Files touched**:
- `crates/lb-l4-xdp/ebpf/src/lib.rs` (add `#[link_section]` static)
- `crates/lb-l4-xdp/src/loader.rs` line 212 (set_license call)
- `crates/lb-l4-xdp/tests/elf_license.rs` (new)

**Joint boundary**: Both files are **ebpf-owned** per §D (EBPF-2-01,
EBPF-2-02 — already covers the ELF license fix). **Sec's role is
limited to a self-downgrade** (medium → low) which the lead applied
in synthesis §A. **The fix work is fully owned by ebpf**; sec's plan
here documents the disposition and the regression test.

**Approach**: ebpf's EBPF-2-02 plan adds:
```rust
#[link_section = "license"]
#[used]
pub static LICENSE: [u8; 4] = *b"GPL\0";
```
Belt-and-suspenders in `loader.rs:212`: change
`EbpfLoader::new().load(elf)` to `EbpfLoader::new().set_license("GPL").load(elf)`.

**Proof**: ebpf writes the proof in EBPF-2-01. Sec adds a
defence-in-depth test in `tests/elf_license.rs`:
`test_compiled_elf_contains_license_section` — uses `goblin` to
parse the built bpfel ELF and asserts a section named `license`
with contents `b"GPL\0"`.

**Cross-ref**: EBPF-2-01, EBPF-2-02.

---

## Combined risk / blast radius

- Three independent, low-surface patches; no cross-coupling.
- Boundaries: sec provides patches + tests; code (SEC-2-08) and
  ebpf (SEC-2-11, SEC-2-12) apply them inside their own plans —
  avoids the "two teammates same file" rule.

---

## Wave-2a follow-up notes (2026-05-13 / 2026-05-14)

### SEC-2-08 — landed in `lb-security`

Sec landed `crates/lb-security/src/key.rs` exposing
`assert_owner_only(path, strict) -> Result<KeyPermAdvice, KeyPermError>`
with seven proof-tests covering 0600 / 0700 lax+strict, 0644 / 0640
lax-advice + strict-error, and missing-file IO error. Status:
`Proposed-Fix(<batch-low sha>)` in `round-2-findings.md`.

The two-line call-site insertion in
`crates/lb/src/main.rs::load_private_key` and
`crates/lb/src/main.rs::load_cert_chain` is **code-owned per §D**
and is handed back to `code` to land alongside CODE-2-05 (TLS
listener accept-loop rewrite). The shape:

```rust
let strict = cfg.runtime.strict_mode;
match lb_security::assert_owner_only(&path, strict) {
    Ok(KeyPermAdvice::Ok) | Ok(KeyPermAdvice::NotApplicable) => {}
    Ok(KeyPermAdvice::TooPermissive { mode }) =>
        tracing::warn!(?path, mode = format!("{mode:#o}"),
                       "key file has loose permissions"),
    Err(e) => return Err(e.into()),
}
```

Same pair of lines in `lb-quic/src/listener.rs:326-337` per the
SEC-2-08 cross-ref. Sec confirms the helper API is stable; bumping
to a different signature is a Round-5 conversation.

### SEC-2-11 — handed to ebpf

`crates/lb/src/xdp.rs` is **ebpf-§D-owned**. The capability-probe
predicate change is a 4-line edit ebpf lands in their own commit
inside EBPF-2-04. Sec's role is the test surface
(`crates/lb/tests/xdp_cap_probe.rs`) which sec also hands to ebpf
because the test crate is rooted in the lb binary (also
ebpf-touched). Sec verified the security rationale in the SEC-2-08
plan is unchanged; ebpf owns landing.

### SEC-2-12 — handed to ebpf

aya 0.13's default loader behaviour is GPL (verified by ebpf in
Round-1). The defence-in-depth `set_license("GPL")` belt-and-
suspenders + the `goblin` ELF-section regression test belong in
ebpf's EBPF-2-02 commit. Sec's only Wave-2a action here was the
disposition note; no code change in `lb-security`.

### SEC-2-14 — closed by L-001

`lb-compression` was removed from the workspace in Round-4 (see
`Cargo.toml` workspace.members comment dated this round). With no
crate to wire, the finding closes by reference. Sec's `round-2-
findings.md` entry now reads `Verified-Fixed(<L-001 sha>)` and
points at the Cargo.toml comment as evidence.
