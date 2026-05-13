# Plan for EBPF-2-04 — XDP attach probe: Native → Drv → SKB ladder + operator knob + telemetry
Finding-ref:     EBPF-2-04 (high, Open)
Files touched:
  - `crates/lb/src/xdp.rs`                  (replace hard-coded `XdpMode::Skb` attach with the ladder; `mode = "skb"` log → `xdp_attach_mode` gauge)
  - `crates/lb-l4-xdp/src/loader.rs`        (`XdpLoader::attach_with_fallback(iface, requested: XdpModeChoice) -> AttachOutcome`)
  - `crates/lb-config/src/lib.rs`           (`RuntimeConfig::xdp_mode: XdpModeChoice`, default `Auto`) — additive, no breakage
  - `crates/lb-l4-xdp/src/stats_export.rs`  (NEW — also used by EBPF-2-08; exports the `xdp_attach_mode` enum value)
  - `crates/lb-l4-xdp/tests/attach_fallback.rs` (NEW — proof test)
  - `CONFIG.md`, `RUNBOOK.md`               (operator docs for the new knob — rel may also edit; serialise per synthesis §D)

Approach:

1. **Config schema (additive)**. Extend `RuntimeConfig`:
   ```rust
   #[derive(Clone, Copy, Debug, Default, Deserialize)]
   #[serde(rename_all = "lowercase")]
   pub enum XdpModeChoice {
       #[default] Auto,    // ladder: Drv → Skb (skip Hw unless explicitly requested)
       Native,             // Drv only; abort startup if unsupported (loud-fail)
       Skb,                // generic SKB (today's behaviour; CI/dev path)
       Hw,                 // hardware offload (mlx5 / nfp only; abort if unsupported)
   }
   ```
   Default `Auto` preserves the principle of least surprise on
   misconfigured systems (a CI box with a `veth` device gets SKB
   automatically) while delivering Drv on real NICs.

2. **Ladder logic in `XdpLoader::attach_with_fallback`**. Replace
   the existing single-call site (`loader.rs:273-275`) with:
   ```rust
   pub fn attach_with_fallback(
       &mut self, prog_name: &str, iface: &str, requested: XdpModeChoice,
   ) -> Result<AttachOutcome, XdpLoaderError> {
       let order: &[XdpMode] = match requested {
           XdpModeChoice::Auto   => &[XdpMode::Drv, XdpMode::Skb],
           XdpModeChoice::Native => &[XdpMode::Drv],          // loud-fail if absent
           XdpModeChoice::Skb    => &[XdpMode::Skb],
           XdpModeChoice::Hw     => &[XdpMode::Hw],           // loud-fail if absent
       };
       for &mode in order {
           match self.xdp_mut(prog_name)?.attach(iface, mode.to_flags()) {
               Ok(_link_id) => {
                   tracing::info!(iface, ?mode, "xdp attached");
                   return Ok(AttachOutcome { mode, attempts });
               }
               Err(e) if is_eopnotsupp(&e) || is_einval(&e) => {
                   tracing::warn!(iface, ?mode, error=%e, "xdp attach unsupported in this mode; trying next");
                   continue;
               }
               Err(e) => return Err(XdpLoaderError::from(e)), // hard error: surfaces immediately
           }
       }
       Err(XdpLoaderError::AllAttachModesExhausted)
   }
   ```
   `is_eopnotsupp` / `is_einval` matches on `aya::programs::ProgramError::SyscallError` whose
   `io::Error::raw_os_error()` is `libc::EOPNOTSUPP` or `libc::EINVAL`.

3. **Wire from `crates/lb/src/xdp.rs:115`**. Replace the hard-coded
   `loader.attach("lb_xdp", iface, XdpMode::Skb)` call with
   `loader.attach_with_fallback("lb_xdp", iface, cfg.runtime.xdp_mode)`.
   On success, increment the telemetry counter (next item).

4. **Telemetry**. Expose the chosen mode via a new
   `stats_export.rs` module (shared with EBPF-2-08):
   ```rust
   // crates/lb-l4-xdp/src/stats_export.rs
   #[derive(Clone, Copy)]
   pub enum AttachModeLabel { Drv, Skb, Hw }
   pub fn record_attach_mode(mode: AttachModeLabel) { /* atomic store of u8 */ }
   pub fn current_attach_mode() -> Option<AttachModeLabel> { /* atomic load */ }
   ```
   `rel` (REL-2-12 / observability crate) reads `current_attach_mode()`
   and exposes the Prometheus gauge:
   ```
   # HELP xdp_attach_mode 1 if XDP is attached in this mode, 0 otherwise
   # TYPE xdp_attach_mode gauge
   xdp_attach_mode{iface="eth0",mode="drv"} 1
   xdp_attach_mode{iface="eth0",mode="skb"} 0
   xdp_attach_mode{iface="eth0",mode="hw"}  0
   ```
   Per-attempt counter (also via `stats_export.rs`):
   ```
   xdp_attach_attempts_total{iface,mode,result="ok|eopnotsupp|einval|other"}
   ```

5. **Loud-fail vs. degrade**. `Native` and `Hw` requests do NOT fall
   back. The loader returns
   `XdpLoaderError::AllAttachModesExhausted`, the binary fails
   `main()`, systemd surfaces the failure. This is the
   defence-in-depth that a 100 G operator wants: better to refuse
   than to silently degrade to 1-3 Mpps.

Proof:

- Test name: `lb-l4-xdp/tests/attach_fallback.rs::ladder_falls_back_on_eopnotsupp`
- Test scaffold:
  1. Create a `dummy0` netdev via `rtnetlink` (the `tests/real_elf.rs`
     fixture already does this — reuse it).
  2. `dummy0` supports SKB-mode XDP but not Drv. Call
     `loader.attach_with_fallback("lb_xdp", "dummy0", XdpModeChoice::Auto)`.
  3. Assert `AttachOutcome.mode == XdpMode::Skb`.
  4. Assert `AttachOutcome.attempts == 2` (Drv then Skb).
  5. Assert `current_attach_mode() == Some(AttachModeLabel::Skb)`.
  6. Negative case: `loader.attach_with_fallback("lb_xdp", "dummy0",
     XdpModeChoice::Native)` MUST return
     `Err(XdpLoaderError::AllAttachModesExhausted)` and NOT attach.
- Marked `#[ignore]` by default (needs CAP_BPF + CAP_NET_ADMIN);
  CI `--ignored` stage runs it.

Risk / blast radius:

- Changing default attach from SKB to Auto-ladder means existing
  dev/CI hosts go from a guaranteed-attach path to a probe path.
  The proof test confirms the SKB fallback works on `dummy0`; on
  any veth-style CI device the ladder lands on SKB just like today.
- `EOPNOTSUPP` detection relies on libc errno constant; aya's
  error variant wraps `io::Error` so `raw_os_error()` is reliable
  across kernel versions.
- The new `XdpModeChoice::Native` knob lets an operator footgun
  themselves into a non-starting binary. Mitigation: clear log
  line + RUNBOOK.md entry instructing operators to set `Auto` if
  unsure.

Cross-ref:
- REL-2-12 / REL-2-13 (rel publishes the gauges via observability).
- EBPF-2-08 (shared `stats_export.rs` module — coordinate file
  ownership: ebpf creates the file in this plan; EBPF-2-08 adds
  the per-CPU STATS surface to it).
- CODE side: `RuntimeConfig` shape change is additive and the
  `code` owner of `crates/lb/src/main.rs` lands it serialised after
  this plan per synthesis §D.

Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
