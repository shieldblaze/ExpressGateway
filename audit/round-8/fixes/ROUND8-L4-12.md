# Plan for ROUND8-L4-12 — XDP detach signature (paired with OPS-04 drain coordinator)

Finding-ref:     ROUND8-L4-12 (medium, Open)
Reference:       kernel selftest `xdp_attach.c` (XDP_FLAGS_REPLACE / BPF_F_REPLACE semantics); xdp-tutorial lesson 12 (multi-program dispatcher); handoff cross-cutting item 1(c) ("reload must explicitly `BPF_F_REPLACE` with the old prog fd or it clobbers third-party programs").
Coverage-gap:    Theme 4 (multi-validator handoff: EBPF-2-04 + CODE-2-10 audited attach mode and drop-safety; multi-program coexistence + `BPF_F_REPLACE` never walked). Bundle B-5 with OPS-04: div-ops authors the drain coordinator plan (single coordinator, two code paths today); this plan supplies the XDP detach + replace signature the coordinator calls.

Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`                (`XdpLoader::query_xdp(iface)`; `XdpLoader::attach_replacing(prog, iface, mode, old_prog_id)`; `XdpLoader::detach_verifying(iface, expected_prog_id)`; new error variants)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (`xdp_attach_query_total{result}` counter: `ok|foreign|none`)
  - `crates/lb-l4-xdp/tests/round8_attach_replace.rs` (NEW — proof, gated `#[ignore]` for kernel-touching cases)
  - `audit/round-8/fixes/ROUND8-OPS-04.md`          (cross-reference — the drain coordinator calls this plan's `detach_verifying` as part of the listener-cancel + XDP-detach atomic step)
  - `RUNBOOK.md`                                    (operator runbook: "before deploying new ExpressGateway, stop the old one fully OR use the soft-reload path")

Approach (XDP detach signature this plan owns):

1. **`query_xdp` accessor** (`crates/lb-l4-xdp/src/loader.rs`):
   ```rust
   pub struct XdpQueryResult {
       pub prog_id: Option<u32>,        // None == no program attached
       pub mode: Option<XdpMode>,
       pub link_id: Option<u32>,
   }
   pub fn query_xdp(iface: &str) -> Result<XdpQueryResult, XdpLoaderError> {
       // Uses aya's wrapper around `bpf_xdp_query` (netlink RTM_GETLINK
       // with IFLA_XDP attribute).
       // ...
   }
   ```
   - `None` `prog_id` means nothing attached.
   - Used by `attach_replacing` (pre-attach probe) and by
     `detach_verifying` (post-detach assertion).

2. **`attach_replacing`** (soft-reload path):
   ```rust
   pub fn attach_replacing(
       &mut self,
       prog_name: &str,
       iface: &str,
       mode: XdpModeChoice,
       old_prog_id: u32,
   ) -> Result<AttachOutcome, XdpLoaderError> {
       // Verify the current attachment is still the one we expect.
       let cur = Self::query_xdp(iface)?;
       match cur.prog_id {
           Some(id) if id == old_prog_id => { /* OK */ }
           Some(id) => return Err(XdpLoaderError::ForeignProgramAttached(id)),
           None => return Err(XdpLoaderError::NoProgramAttached),
       }
       // Set BPF_F_REPLACE in XdpFlags; aya 0.13.x exposes
       // `XdpFlags::REPLACE` and an `attach_to_link_id` style API.
       // ...
   }
   ```
   - The non-replace path (`attach_with_fallback`) remains the
     primary entry point; `attach_replacing` is only used by the
     drain coordinator's soft-reload step.

3. **`detach_verifying`** (the half OPS-04's drain coordinator
   calls):
   ```rust
   pub fn detach_verifying(
       &mut self,
       prog_name: &str,
       iface: &str,
       expected_prog_id: u32,
   ) -> Result<(), XdpLoaderError> {
       // First check current attachment matches what we expect.
       let pre = Self::query_xdp(iface)?;
       match pre.prog_id {
           Some(id) if id == expected_prog_id => { /* OK */ }
           Some(id) => return Err(XdpLoaderError::ForeignProgramAttached(id)),
           None => return Err(XdpLoaderError::NoProgramAttached), // someone already detached
       }
       // Drop our Xdp handle (aya detaches when the link is dropped).
       let program = self.ebpf.program_mut(prog_name).ok_or(...)?;
       let xdp: &mut Xdp = program.try_into().map_err(...)?;
       xdp.detach()?;
       // Verify the kernel sees no program attached now.
       let post = Self::query_xdp(iface)?;
       if post.prog_id.is_some() {
           return Err(XdpLoaderError::DetachLeftProgramAttached {
               iface: iface.to_owned(),
               prog_id: post.prog_id,
           });
       }
       Ok(())
   }
   ```
   - This is the **detach signature** OPS-04's drain coordinator
     promises to call. The coordinator's contract:
     1. Cancel listener accept loops (OPS-04 owns).
     2. Drain in-flight per-connection tasks (OPS-04 owns).
     3. Call `loader.detach_verifying(prog, iface, our_prog_id)`
        as the FINAL drain step. (This plan owns.)
     4. Verify `xdp_attach_mode` gauge drops to 0. (rel's
        observability owns; trivially correct once detach returns OK.)
   - Coordinator failure modes: `ForeignProgramAttached`
     (operator-error or competing tool — alert + leave the program
     alone), `NoProgramAttached` (already detached, idempotent — log
     INFO, continue drain), `DetachLeftProgramAttached` (kernel
     bug — alert ERR, force `ip link set dev <iface> xdp off`).

4. **New error variants**:
   ```rust
   #[error("foreign XDP program attached to {0}: prog_id={1}; refusing to attach")]
   ForeignProgramAttached(u32),
   #[error("no XDP program attached")]
   NoProgramAttached,
   #[error("detach left a program attached on {iface:?}: prog_id={prog_id:?}")]
   DetachLeftProgramAttached { iface: String, prog_id: Option<u32> },
   ```

5. **Metric** (`crates/lb-l4-xdp/src/stats_export.rs`):
   - `xdp_attach_query_total{iface, result="ok|foreign|none"}` —
     incremented by `query_xdp` callers.
   - `xdp_detach_total{iface, result="ok|foreign|leak"}` —
     incremented by `detach_verifying`.

6. **Proof tests** (`crates/lb-l4-xdp/tests/round8_attach_replace.rs`,
   NEW):
   - `query_returns_none_for_clean_iface` (`#[ignore]`): create a
     dummy iface; `query_xdp("dummy0")` returns `prog_id: None`.
   - `query_returns_our_prog_after_attach` (`#[ignore]`):
     attach -> query returns `prog_id == ours`.
   - `attach_replacing_succeeds_when_id_matches` (`#[ignore]`):
     attach prog A -> capture id_A -> `attach_replacing(prog_B, iface, mode, id_A)`
     -> `query_xdp` returns id_B.
   - `attach_replacing_fails_when_foreign_attached` (`#[ignore]`):
     attach prog X via `bpftool net attach` -> our
     `attach_replacing` returns `Err(ForeignProgramAttached(_))`.
   - `detach_verifying_succeeds_clean` (`#[ignore]`): attach,
     detach via API, `query_xdp` returns None.
   - `detach_verifying_idempotent_after_external_detach`
     (`#[ignore]`): attach, `bpftool net detach`, our
     `detach_verifying` returns `NoProgramAttached` (treated as OK
     by the drain coordinator).
   - `detach_signature_matches_ops04_coordinator` (NOT ignored —
     interface-level check): assert
     `loader.detach_verifying(prog, iface, expected_id)` exists
     with the documented signature. This is the cross-plan
     contract assertion OPS-04 also calls in its plan tests.

7. **RUNBOOK update**:
   - New section: "Restarting ExpressGateway":
     - Soft reload: `systemctl reload expressgateway` (calls
       `attach_replacing`).
     - Hard restart: `systemctl restart expressgateway` (drain ->
       detach -> attach).
     - Concurrent-tool conflict: if another XDP tool is attached,
       restart fails with `ForeignProgramAttached`. Operator must
       resolve manually (`bpftool net detach <iface>`).

Proof:

- `cargo test -p lb-l4-xdp --test round8_attach_replace`
  (non-ignored signature-contract test).
- `cargo test -p lb-l4-xdp --test round8_attach_replace -- --ignored`
  (kernel-touching tests on the privileged CI lane).
- Cross-plan: the OPS-04 drain coordinator plan's proof test calls
  `loader.detach_verifying(...)` with the same signature.
- Re-capture verifier-log baselines via L4-10 — but no BPF source
  change here, so the diff should be empty.

Risk / blast radius:

- `query_xdp` uses netlink; aya wraps `bpf_xdp_query` already.
  Adds one syscall per attach/detach — negligible.
- `attach_replacing` with the wrong `old_prog_id` returns
  `ForeignProgramAttached` — surface the running prog_id in the
  error so the operator can decide.
- If the previous process exited uncleanly, the kernel still has
  the link until the link fd's last reference drops. The
  ID-based replace path correctly identifies "our" prog by id.
  Edge case: a CI reboot between attach and detach — handled by
  the soft-reload path (a fresh start with no expected id falls
  back to `attach_with_fallback`).

Cross-ref:
- Bundle B-5 peer: OPS-04 (drain coordinator). OPS-04 author
  consumes this plan's API surface.
- `audit/ebpf/round-2-review.md` EBPF-2-04 (attach mode): status
  amended to note multi-program coexistence is now handled.
- ROUND8-L4-05 (post-attach probe): `attach_replacing` should ALSO
  run the L4-05 probe after a successful replace, so a silent-drop
  NIC class is caught on reload too. Sequence: L4-05 lands first;
  this plan's `attach_replacing` reuses the probe helper.

Owner:           div-l4 (XDP detach signature); div-ops (drain coordinator plan, via OPS-04)
Lead-approval: approved 2026-05-14 team-lead-r8
