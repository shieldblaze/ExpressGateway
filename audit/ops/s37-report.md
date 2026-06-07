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

## Phase 2 — (C) hot reload `[reload-eng]` — ✅ VERIFIED + INTEGRATED (merge `ad87e65e`)

**C v1 = SIGHUP validate-first / atomic-swap L7 hot reload** (final commit `6e43b09c`). SIGHUP → full `lb_config::validate_config` → `LbConfig::diff`/`ReloadPlan` → atomic `Arc<ArcSwap>` per-listener proxy swap (mirrors the proven SIGUSR1+ArcSwap cert pattern). Snapshot-at-accept: each connection captures one owned `Arc<H1Proxy/H2Proxy>` at accept → in-flight isolation by construction.
- **Swappable (applied live, summary-honest):** backends/weights, http timeouts, `max_keepalive_requests`, `h2_security`, websocket H1/H2 knobs, `alt_svc`, `grpc`. TLS cert/key via the existing SIGUSR1 path.
- **Restart-required (honest WARN+metric, never silent):** `per_ip_connection_cap` + `strict_te` (shared HooksBundle, not proxy state — carried as the deeper-swap follow-up), QUIC/H3 backends (CF-S37-C-H3-BACKEND-RELOAD), listener add/remove/bind/protocol, XDP, TLS cert PATH/ALPN.

**Independent verification — C is PROMOTABLE (live-connection preservation PROVEN under traffic):**
- Crown-jewel proofs (verifier, real running gateway, completed runs): reload-under-traffic suite **9/9**; **gRPC mid-RPC survives** (frame after SIGHUP still served by the original backend, grpc-status 0), **live WS tunnel survives**, mid-stream chunked response survives (10 chunks, 0 leak); **invalid-reload-no-blip** (+ `config_reload_failed_total` bump); **restart-required-honesty** (no silent rebind + WARN); **no-cross-talk**; **reload-takes-effect**; **`reload-summary-matches-observed-behavior` bidirectional, every field class** (incl. co-changed backend+timeout — the inverse-dishonesty bug is gone). Verifier **could not defeat isolation or honesty**. Evidence: `audit/ops/s37-verify-C/`.
- **F-S37-C-1** (pre-existing MED signal-loss: TERM-after-HUP/USR1 dropped) **FIXED** — `LifecycleSignals` installed once before the loop; re-verified HUP→TERM **0/6 stuck** (was 3/6), USR1→TERM 0/6. Regression test `signal_loss_terminal_after_nonterminal_still_drains`.
- **F-S37-C-2** (LOW, doc-only): stale `rebuild_l7_proxies` comment — **fixed at integration** (this branch).
- **Binding ×3 (R3-after-seam / no-regression):** run2 clean **1562/0/18**; run1/run3 failures were the known throughput flakes (`fcap1_h2`, its `h2h3` sibling, `t5` = CF-FCAP1-FLAKE / fcap1-family / CF-S35-T5-FLAKE), each **isolation-confirmed PASS 3/3** serialized off-load (R2 — not regressions). clippy/fmt clean. Coverage: `lb-config/src/lib.rs` 90.42%, `reload.rs` 83.78% (≥80 charter). Evidence: `audit/ops/s37-verifyC-lead/`.

### Phase 2 — (C) plan (as executed)
SIGHUP trigger · validate-first via full `lb_config::validate_config` · `Arc<ArcSwap<EffectiveConfig>>` mirroring the SIGUSR1 cert pattern · snapshot-at-accept (in-flight preserved, new conns get new config, no cross-talk) · honesty rule (restart-required change → clear per-field warning/reject, never silent).

**R13 load-bearing proofs (real running gateway under traffic) — SIX:**
1. live-connection-survives-reload (gRPC mid-RPC + live WS complete across reload) + **negative control** (naive swap would reset them).
2. invalid-reload-no-blip (bad config → keeps serving old, zero interruption).
3. restart-required-honesty (bind-addr change → clear detected response; bind did NOT silently change; uninterrupted).
4. no cross-talk (old-config vs new-config conns not mis-routed).
5. **(owner addition) reload-TAKES-EFFECT** — after SIGHUP changing a swappable field (backend weight/timeout), NEW connections observe the new value (guards against a no-op reload passing 1–4).
6. **(owner addition) R3-after-seam (structural, verifier-independent)** — after the baked-read→snapshot-read conversion, the existing e2e/soak suite passes UNCHANGED. The risk is the *non-reload* data path subtly changing, not the reload logic. Load-bearing.

If live-connection-preservation can't be PROVEN under traffic → C NOT promoted (R11), carried. Check-in to owner with the proof + negative control before integration.

**Phase-2 check-in (owner decision, 2026-06-07):** C increment 1 (commit `78c18dd6`) delivered the SIGHUP validate-first reload + atomic `Arc<ArcSwap>` seam + 5/5 H1 wire proofs + an in-process negative control. reload-eng honestly surfaced two gaps; owner ruled:
- **Scope = L7 v1** (H1/H1s carrying H1/H2/gRPC/WS). **QUIC/H3 backend hot-reload stays RESTART-REQUIRED this session (honest WARN+metric), CARRIED** as its own scoped increment — it interacts with the S36 cap→GOAWAY→recycle lifecycle (swapping the H3 upstream pool mid-recycle is a distinct live-survival surface needing its own focused proof). → **CF-S37-C-H3-BACKEND-RELOAD** (carry).
- **Non-waivable before C promotes:** (1) gRPC mid-RPC + live WS survival PROVEN on the wire (the hard stateful cases — H1-only insufficient). (2) Diff-honesty BOTH directions via making the full L7 swappable set genuinely applied + classified swappable; **new required proof `reload-summary-matches-observed-behavior` for EVERY field class** (a field reported "applied" observably takes effect; one reported "restart-required, not applied" observably does not — guarding the inverse-dishonesty bug). (3) `config_reload_failed_total` metric assertion in invalid-no-blip.
- Mechanism (SIGHUP) confirmed. reload-eng executing the additions; verifier held until the final C commit, then the binding run.

**Interim independent verification (verifier, on `78c18dd6`) — cross-validated reload-eng's gaps + 1 new finding:**
- Verifier's beyond-suite wire proofs (all PASS): h1s/H2 ALPN backend swap takes effect; an **in-flight mid-stream chunked response SURVIVES the reload** (10 old-backend chunks intact, 0 new-backend leak — the closest wire analog to gRPC/WS stream survival, confirming the captured-snapshot mechanism holds for long-lived streams); mixed swappable+restart-required reload (backend applied + handshake_timeout warned restart-required); restart-required-only (applied=0 + honest WARN + no silent bind). Could NOT make any restart-required change silently apply.
- Confirmed gaps → routed back to reload-eng: (i) suite covers only plain-H1 discrete GETs (no gRPC/WS/h1s reload test); (ii) **diff-honesty mismatch** pinned by code-read — `rebuild_l7_proxies` rebuilds the whole proxy from new config, so http/h2_security/websocket ARE applied when co-changed with a backend while `diff_listener` reports them restart-required → fix via option A (full L7 swappable).
- **F-S37-C-1 (NEW, PRE-EXISTING, MED):** a SIGTERM/SIGINT landing right after a non-terminal signal (SIGHUP or pre-existing SIGUSR1) can be silently lost — `wait_for_lifecycle_signal()` re-installs the unix_signal streams every loop iteration (main.rs ~2940/3239-3288); a terminal signal in the drop→recreate window isn't redelivered. Proven pre-existing in the integrated-B baseline (SIGUSR1+SIGTERM repro). Recoverable (2nd SIGTERM always drains). **Fix folded into C** (install the 4 streams once before the loop + regression test) — tractable correctness (R6); C makes it more reachable via SIGHUP.

## Phase 3 — (D) latest-deps upgrade — ✅ EXECUTED + INTEGRATED (`4767fb2c`, lead-executed per dep-eng plan)

dep-eng authored the plan; on execution it idled after each command, so per R9 the lead drove the stages (verifier independently re-validates → author≠verifier holds). **Upgraded (direct):** socket2 0.6.3→0.6.4, **prometheus 0.13.4→0.14.0** (the held un-hold — compiles clean; needed a 3-site `get_name()`→`.name()` adaptation in lb-observability, the only source change), object 0.37.3→0.39.1; `time`→0.3.47 (transitive). **Already-latest no-ops:** quiche/tokio-quiche/hyper/h2/tokio-tungstenite/rustls. **Held + carried:** **tokio 1.51.1 (NOT bumped to 1.52 — see finding below; pinned `<1.52`)**, foundations 4.5.0 (held-by-upstream: `quiche→qlog→foundations ^4`; pins the prometheus-0.13.4/object-0.37.3 transitive duals), aya 0.13.1 (0.13.2 yanked), **reqwest 0.12** (0.13 dropped `rustls-tls` feature + aws-lc default — dev-only → `CF-S37-D-REQWEST-0.13`), idna_adapter 1.1.0. **Security:** `time` 0.3.47 → dropped RUSTSEC-2026-0009 (cargo audit 0-vuln confirms); RUSTSEC-2024-0320 kept (foundations-capped); "1.85"→"1.88" prose fixed. **Supersedes PR #224 (folded; tokio member dropped per finding) + #226 (Actions YAML applied).** **hyper#4050 not fixed → WS-H2 stays gated.**

**FINDING — F-S37-D-1 / CF-S37-D-TOKIO-1.52-RELAY (Phase-4 caught a real regression):** tokio **1.52.3 regresses the H2→H3 relay throughput ~10×**. `tests/h2h3_md_streaming_verify.rs::h2h3_fcap1_over_cap_upload_never_complete` forwards only ~30 MiB in its 60s window on 1.52.3 (the over-cap arm never reached → vacuity FAIL) vs **64 MiB in ~5.7s on 1.51.1 (PASS, cap enforced at exactly 64 MiB)**. Reproduced **3/3 on a QUIET box** (load 0.25), then **isolation-confirmed the fix 3/3 after reverting tokio→1.51.1** (5.7s pass). Passed on Phase-0 main + B+C; only B+C+D regressed it → attributed to tokio (socket2 0.6.4 has no Linux change; prometheus/object don't touch the relay). Root cause: tokio 1.52.0 reverted the LIFO slot-stealing scheduler optimization, reshaping relay task scheduling. **Resolution (R6):** hold tokio at 1.51.1 (the proven baseline that main ships), pin `<1.52`, carry the 1.52 bump (CF-S37-D-TOKIO-1.52-RELAY) for upstream follow-up. The rest of D (prometheus 0.14 + object 0.39 + socket2 + time + Actions + MSRV) stays. Re-validation re-run on the corrected (tokio-1.51.1) tree = Phase 4 below.

### Phase 3 — (D) plan (as executed)
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
