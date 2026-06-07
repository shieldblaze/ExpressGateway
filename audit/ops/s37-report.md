# Session 37 — Config management + hot reload + latest-deps upgrade

Branch: `feature/ops-bcd-s37` (off main `21ee3c65`, S36-A H3-recycling promoted, leak FIXED, CI honest-green).
Workstreams: **(B)** protocol-complete config management/validation, **(C)** validate-first / drain / atomic-swap hot reload (SIGHUP), **(D)** latest-deps upgrade.

Owner: hyperxpro. Box: 8 cores, 15 GiB RAM + 8 GiB swap (added Phase 0). Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`, `CARGO_BUILD_JOBS=4`.

---

## Phase 0 — baseline + swap + watchdog (COMPLETE)

| Item | Result |
|---|---|
| Base tip | main `21ee3c65` (S36 H3 recycling — close CF-GRPC-H3-CHURN-RSS) ✓ |
| Branch | `feature/ops-bcd-s37` created off `21ee3c65`, pushed to origin ✓ |
| Swap | **8 GiB** swapfile created + enabled (`/swapfile`) — was 0B (15 GiB OOM cushion) ✓ |
| Strays | none (clean `ps aux`) ✓ |
| Disk | 38G free at start; full `--all-features` build → 11G (CF-DISK-1; reclaim between heavy phases) |
| ×3 gate | **1522 / 0 / 18** all three runs (`audit/ops/s37-phase0/`) ✓ |
| fmt | clean ✓ |
| clippy `--all-targets --all-features -D warnings` | clean ✓ |
| sc9-BOUNDED | confirmed from completed S36 run (`audit/soak/s36-soak-data/sc9-postfix/`): rss/vmhwm/fds/threads BOUNDED (~22 MB plateau, vs 80 MB pre-fix DRIFT), panic=0, 151 samples / 1800s, 5M+ RPCs ✓ |

Baseline matches the S36 reference (1522/0/18) exactly — the foundation B/C build on is intact. Full CI conformance gate set (h2spec 147/147, h3spec 12-waiver, D6 per-module ≥80%, docker-smoke, XDP-verifier) was promoted honest-green on `21ee3c65` (S36); re-confirmed in Phase 4.

### Reconnaissance (read-only, pre-plan)
Two deep maps produced (config/boot/reload surface; deps/harness surface). Key facts driving the plan:
- **B headline gap:** no `#[serde(deny_unknown_fields)]` anywhere + 88 `#[serde(default)]` ⇒ typos / unknown keys silently swallowed. `lb_config::validate_config` is otherwise a real fail-loud validator. `http`/`h2`/`h3` listener protocols pass validation but are NOT served by the binary (only `quic` vs tcp-family branch). `max_keepalive_requests`/`max_requests_per_h3_connection` have no range check.
- **C surface:** no SIGHUP handler, no config hot-swap, no admin reload route. `lb-controlplane::ConfigManager` exists but is **inert** (`let _config_manager`, "Wave-2"); its `validate()` is TOML-shape-only (NOT `lb_config::validate_config`). Proven hot-swap pattern in-repo: TLS certs via SIGUSR1 + `Arc<ArcSwap<TlsConfigBundle>>` (mirror this, R12). No gateway-builder seam — listeners spawned inline (`crates/lb/src/main.rs:2330-2389`).
- **D surface:** quiche 0.29.1, tokio-quiche 0.19.0, hyper 1.10.1, h2 0.4.14, tokio 1.51.1, prometheus **0.13.4 (held)**, tokio-tungstenite 0.29.0, rustls 0.23.40. No `[patch]`, no git deps. PR #224 (tokio/socket2/prometheus/object) + #226 (actions) to fold. hyper#4050/WS-H2 gate at `crates/lb-l7/src/h2_proxy.rs:841`, config flag `h2_extended_connect` default-false (CF-S27-2).

### Owner product decisions (Phase-2 reload design)
- **Reload trigger = SIGHUP** (zero new attack surface; ConfigManager pre-wired for it; mirror the SIGUSR1+ArcSwap cert pattern). Validate-first uses the **full `lb_config::validate_config`**, not ConfigManager's TOML-shape `validate()`.
- **Reload scope = config swap, ports stable.** Swappable via atomic ArcSwap: backends/weights, timeouts, limits (`max_keepalive_requests`, `max_requests_per_h3_connection`), per-ip caps, H2/WS security knobs, TLS cert/key. **Restart-required** (documented): listener add/remove, bind-address change, listener protocol change, XDP enable/disable.
- **Non-negotiable honesty rule:** a SIGHUP whose new config changes a restart-required field must be **DETECTED and explicitly handled** (apply swappable + clear per-field "requires restart, not applied" warning, or reject) — **never a silent no-op that looks successful**. R13 covers BOTH the positive proof (live conns incl. gRPC mid-RPC + live WS survive) AND the honesty proof (restart-required change → clear detected response).

---

**Plan APPROVED** (owner, Phase 0). Team `s37-ops`: lead + config-eng (B) + reload-eng (C) + dep-eng (D) + verifier. Sub-branches off `feature/ops-bcd-s37`, integrated B→C→D; verifier independent (author≠verifier); promote `--no-ff` per R11.

## Phase 1 — (B) config management `[config-eng]` — ✅ VERIFIED + INTEGRATED (merge `47744ffa`)

**Built (config-eng, `a3da4db2`):** `#[serde(deny_unknown_fields)]` on all 20 Deserialize structs (2 enums inapplicable; **zero `serde(flatten)`** so it applies uniformly); reject `http`/`h2`/`h3` as listener protocols (served set tcp/tls/h1/h1s/quic; clear message routing H2→h1s/ALPN, H3→quic); range-check `max_keepalive_requests` + `max_requests_per_h3_connection` (0=disable, else 1..=10_000_000); `tests/config_boot_matrix.rs` (real-binary boots + negative boots); CONFIG.md rewrite; `config/examples/*.toml` ×9.

**Verified (independent, author≠verifier):**
- verifier real-wire (`origin/s37-verify @ 44fbd7fd`): all 9 example configs boot+bind; h1 path serves 200 end-to-end; every invalid class rejects loud (exit 1, actionable msgs — e.g. unknown key names the expected field; unserved-protocol explains the served set); adversarial stray-key-in-every-nested-block all reject; range boundaries exact (==10M ok, 10M+1 rejects); default.toml + docker/smoke boot under deny_unknown_fields. **Zero findings.**
- binding ×3 (lead takeover after verifier's run1 stalled on an unbounded monitor-wait, R9; lead independent of B's author): run1 1539/**1**/18, run2 **1540/0/18**, run3 **1540/0/18** — sole failure was `fcap1` in run1, passed clean runs 2&3 → confirmed CF-FCAP1-FLAKE (R2), not a defect. clippy + fmt clean. **lb-config/src/lib.rs coverage 90.42%** (≥80 charter). Evidence: `audit/ops/s37-verifyB-lead/`.
- R3 byte-identical: equivalent-to-today configs unchanged; existing TOML-boot e2e/soak green under deny_unknown_fields; only a test using `protocol="http"`→`tcp` + the u32-range cap test rewritten to the 10M bound (no validation weakened).

### Phase 1 — (B) plan (as executed)
1. `#[serde(deny_unknown_fields)]` on `LbConfig` + every nested struct (single-sourced) — close the silent-typo gap. Negative tests per error class.
2. Reject `http`/`h2`/`h3` listener protocols (accepted today, never served) with a clear message → served set `tcp`/`tls`/`h1`/`h1s`/`quic`.
3. Range checks for `max_keepalive_requests`, `max_requests_per_h3_connection` (0=disable), + other unchecked numeric knobs.
4. R3 byte-identical: equivalent-to-today configs boot + existing TOML-boot e2e/soak pass unchanged; new real-binary boot tests (each listener type) + invalid-config negative tests (clear messages).
5. Refresh `CONFIG.md`; `config/examples/` per listener type. Gate scoped ×3 + clippy + fmt; `lb-config` cov ≥ charter.

## Phase 2 — (C) hot reload `[reload-eng]` — correctness-critical _(staged behind B)_
SIGHUP trigger · validate-first via full `lb_config::validate_config` · `Arc<ArcSwap<EffectiveConfig>>` mirroring the SIGUSR1 cert pattern · snapshot-at-accept (in-flight preserved, new conns get new config, no cross-talk) · honesty rule (restart-required change → clear per-field warning/reject, never silent).

**R13 load-bearing proofs (real running gateway under traffic) — SIX:**
1. live-connection-survives-reload (gRPC mid-RPC + live WS complete across reload) + **negative control** (naive swap would reset them).
2. invalid-reload-no-blip (bad config → keeps serving old, zero interruption).
3. restart-required-honesty (bind-addr change → clear detected response; bind did NOT silently change; uninterrupted).
4. no cross-talk (old-config vs new-config conns not mis-routed).
5. **(owner addition) reload-TAKES-EFFECT** — after SIGHUP changing a swappable field (backend weight/timeout), NEW connections observe the new value (guards against a no-op reload passing 1–4).
6. **(owner addition) R3-after-seam (structural, verifier-independent)** — after the baked-read→snapshot-read conversion, the existing e2e/soak suite passes UNCHANGED. The risk is the *non-reload* data path subtly changing, not the reload logic. Load-bearing.

If live-connection-preservation can't be PROVEN under traffic → C NOT promoted (R11), carried. Check-in to owner with the proof + negative control before integration.

## Phase 3 — (D) latest-deps upgrade `[dep-eng]` _(concurrent; integrated last)_
Staged: patch group → prometheus 0.13→0.14 (held un-hold) + object→0.39 (folds #224) → quiche/tokio-quiche→latest [h3spec vs 12-waiver] → hyper/h2→latest [h2spec 147/147; **hyper#4050: un-gate WS-H2 ONLY IF the fix is in the adopted release — re-prove the backpressure plateau as the un-gate gate; else WS-H2 stays gated**] → tokio-tungstenite→latest [WS matrix]. Supersede #224/#226. R8 re-proven. MSRV bump only if latest quiche requires (+ refresh stale "1.85" prose). Un-verifiable crate → drop/pin-back, carry.

**D research COMPLETE** (`audit/ops/s37-deps-plan.md`, committed `5470a4f5`) — far smaller than feared:
- **Already at latest (no bump available, relock/no-op):** quiche 0.29.1, tokio-quiche 0.19.0, hyper 1.10.1, hyper-util, h2 0.4.14, tokio-tungstenite 0.29, rustls 0.23.40, tokio-rustls, http, serde/json, toml, bytes, etc. ⇒ **the H2/H3/WS protocol-lib surface is UNCHANGED by D** (h2spec/h3spec re-run is a no-regression formality, not a version shift).
- **Real bumps (all mechanical, zero source change predicted):** tokio 1.51.1→**1.52.3**, socket2 0.6.3→**0.6.4**, prometheus 0.13.4→**0.14.0** (AsRef widening, `process` feature retained, default-features-off keeps protobuf out), object 0.37.3→**0.39.1** (read-only ELF path untouched), aya 0.13.1→0.13.2, aya-obj 0.2.1→0.2.2, idna_adapter 1.1.0→1.2.2.
- **Best-effort / hold:** foundations 4.5→5.7 (transitive; **likely capped at ^4 by tokio-quiche 0.19** → leave 4.5.0 as held-by-upstream, a finding not a failure; if it moves + drops serde_yaml, prune RUSTSEC-2024-0320 waiver); reqwest 0.12→0.13 (**dev-only major**; hold at 0.12.28 if aws-lc/ring crypto-provider default fights us).
- **hyper#4050 VERDICT (primary-sourced):** **NOT fixed in any release** — fetched `v1.10.1/src/proto/h2/upgrade.rs`, the buggy `Poll::Pending => break 'capacity` fall-through is still present; PR #4050 is open/unmerged. ⇒ **WS-H2 stays gated, `#[ignore]` stays, CF-S27-2 stays open (upstream), no fork.** Owner's un-gate rule not met.
- MSRV stays 1.88 (highest requirement among bumps is object-all-features 1.87). Stale "1.85" prose in `deny.toml`/`.cargo/audit.toml` to refresh (Stage 6).
- Execution staged behind B integration (dep-eng on stand-by) to avoid dual-lockfile thrash on the shared target.

## Phase 4 — full re-validation + promote `[verifier + lead]`
×3 · h2spec 147/147 · h3spec 12-waiver · WS matrix · gRPC-H3 · R8+R13 · full re-soak ALL BOUNDED (sc9 ~22MB) · docker-smoke from config · all CI gates · per-module cov. Promote `--no-ff` per R11.
