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

## Phase 1 — (B) config management
_(pending plan approval)_

## Phase 2 — (C) hot reload
_(pending plan approval)_

## Phase 3 — (D) latest-deps upgrade
_(pending plan approval)_

## Phase 4 — full re-validation + promote
_(pending)_
