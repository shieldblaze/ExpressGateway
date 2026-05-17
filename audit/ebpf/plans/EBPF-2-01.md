# Plan for EBPF-2-01 — Emit `license` ELF section + `BTF` / `BTF.ext` in the eBPF binary
Finding-ref:     EBPF-2-01 (high, Open) — folds in EBPF-2-02 (medium, Open) per lead instruction; shared files, single source patch.
Files touched:
  - `crates/lb-l4-xdp/ebpf/src/main.rs`  (add `#[link_section = "license"]` static; this is the EBPF-2-02 mechanism)
  - `scripts/build-xdp.sh`               (bpf-linker invocation: emit `-g` / `--btf`; verify sections via `readelf`)
  - `crates/lb-l4-xdp/build.rs`          (call `scripts/build-xdp.sh`; size-drift guard against committed `src/lb_xdp.bin`)
  - `crates/lb-l4-xdp/src/lb_xdp.bin`    (regenerated artefact; **byte-size budget enforced — see below**)
  - `crates/lb-l4-xdp/tests/elf_sections.rs`  (NEW — proof test, see Proof)
  - `crates/lb-l4-xdp/Cargo.toml`        (dev-dep on `goblin` or `object` for ELF inspection in the proof test)
  - `audit/ebpf/plans/EBPF-2-02.md`      (cross-pointer: redirects to this file — they are merged here)

Approach:

1. **License section (closes EBPF-2-02 mechanism)**. In
   `crates/lb-l4-xdp/ebpf/src/main.rs`, immediately under the
   `#![no_std] #![no_main]` preamble, add:
   ```rust
   #[unsafe(link_section = "license")]
   #[unsafe(no_mangle)]
   pub static LICENSE: [u8; 4] = *b"GPL\0";
   ```
   `unsafe(link_section …)` is required on the 2024-edition / nightly
   toolchain the eBPF crate uses (`rust-toolchain.toml` in
   `crates/lb-l4-xdp/ebpf/`). The `\0` terminator is mandatory — the
   kernel `bpf_attr.license` field is a C string and aya-obj-0.2.1
   `parse_license()` rejects non-NUL-terminated bytes.

2. **BTF emission**. `scripts/build-xdp.sh` today invokes
   `bpf-linker` without `-g`. Update the link invocation to:
   ```
   bpf-linker \
     --emit=obj \
     -g \
     --btf \
     -O 2 \
     --target=bpfel-unknown-none \
     -o "$OUT" \
     "$@"
   ```
   `-g` emits DWARF; `bpf-linker` lowers DWARF→BTF when `--btf` is
   set. The compiler driver (`cargo +nightly rustc -Z build-std`) must
   also pass `-Cdebuginfo=2` for the DWARF input to exist. Wrap the
   whole flow as `cargo xtask build-xdp` if/when the workspace gains
   an `xtask` crate; until then, `scripts/build-xdp.sh` is the
   canonical entry point and `build.rs` shells out to it.

3. **`maps` section migration is OUT OF SCOPE** for this plan
   (separate finding territory). We accept that `bpftool gen
   skeleton` will still fail post-fix; the goal here is GPL
   declaration + BTF for `bpftool prog dump xlated` readability,
   not full libbpf-1.0 skeleton compatibility.

4. **Byte-size drift guard**. After the rebuild, the committed
   `lb_xdp.bin` will grow (BTF adds ~2-8 KiB; license adds 4 B). To
   prevent unbounded drift on future eBPF source changes, add to
   `build.rs`:
   ```rust
   const MAX_ELF_BYTES: u64 = 64 * 1024;   // 64 KiB ceiling
   ```
   and have the proof test assert `metadata(lb_xdp.bin).len() < MAX_ELF_BYTES`.
   The current pre-fix binary is 8168 bytes; post-fix expected ~12-16
   KiB. 64 KiB ceiling gives ~4× headroom and forces a deliberate
   review on any future growth.

5. **CI dependency**. `bpf-linker` is not in the default Rust sandbox.
   CI must install it once: `cargo install bpf-linker --locked`.
   Document this in `audit/ebpf/plans/EBPF-2-07.md` (verifier matrix)
   since that plan owns the CI image.

Proof:

- Test name: `lb-l4-xdp/tests/elf_sections.rs::elf_has_license_btf_and_size_budget`
- Invariants asserted (using the `object` crate):
  1. `goblin::elf::Elf::parse(bytes).section_headers` contains a
     section named `"license"` whose bytes equal `b"GPL\0"`.
  2. The ELF contains a section named `".BTF"` with non-zero size.
  3. The ELF contains a section named `".BTF.ext"` with non-zero size.
  4. `std::fs::metadata(LB_XDP_BIN_PATH).len() < 64 * 1024`.
- Out-of-band reproduction (captured by `xtask`/`build.rs` post-build
  step and printed in CI logs):
  ```
  readelf -p license  crates/lb-l4-xdp/src/lb_xdp.bin   # → "GPL"
  readelf -S          crates/lb-l4-xdp/src/lb_xdp.bin | grep -E '\.BTF(\.ext)?'
  stat -c '%s'        crates/lb-l4-xdp/src/lb_xdp.bin   # → <65536
  ```
  The CI step `xtask xdp-verify` (EBPF-2-07) greps for these lines
  and fails on mismatch.

Risk / blast radius:

- `unsafe(link_section)` on a static that the eBPF entry-point does
  not reference: bpf-linker's dead-code elimination may strip it.
  Mitigation: `#[unsafe(no_mangle)]` keeps the symbol alive; if DCE
  still strips it, fall back to a `static mut` that is touched from
  the entry function with a `core::hint::black_box`. The proof test
  catches this regression by failing if the section disappears.
- BTF emission may surface previously-hidden verifier complaints
  (the verifier uses BTF for tighter type checks). Mitigation:
  EBPF-2-07's per-kernel verifier-log matrix catches that the
  moment it's merged.
- Committed-binary churn: every eBPF-source change now produces a
  larger diff. Reviewers must accept this; the 64 KiB ceiling
  bounds the noise.

Cross-ref:
- Closes EBPF-2-02 (folded — same source file, same mechanism).
- Unblocks EBPF-2-07 (BTF-enriched verifier logs are more readable).
- Sec SEC-2-12 has been downgraded to low based on the
  aya-obj-0.2.1 `"GPL"` default; this plan still ships the explicit
  section so SEC-2-12 closes for-real.

Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
