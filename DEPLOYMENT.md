# Deployment

This document covers running the `expressgateway` binary on Linux in a
production-adjacent shape. The compiled XDP BPF ELF ships in-tree and the
userspace loader attaches it (see
[XDP toolchain & the shipped BPF ELF](#xdp-toolchain--the-shipped-bpf-elf));
XDP is off by default and single-kernel (validated against a specific
kernel window — see [Kernel floor](#kernel-floor)).

## Build

```
cargo build --release -p lb --bin expressgateway
```

Artifact: `target/release/expressgateway`. Strip is enabled by the
`[profile.release]` block in root `Cargo.toml`; LTO is set to `thin`.
The crate is named `lb` but the binary it produces is `expressgateway`
(see `crates/lb/Cargo.toml` `[[bin]] name = "expressgateway"` and
`docker/Dockerfile` line 31 `COPY .../target/release/expressgateway
/usr/local/bin/expressgateway`).

### Build-time dependencies

- **`cmake`** (≥ 3.20) — required because `quiche` links BoringSSL and BoringSSL's build is driven by cmake. `cmake 3.28` confirmed to work on Ubuntu 24.04. See ADR `docs/decisions/quinn-to-quiche-migration.md`.
- **C/C++ toolchain** (`build-essential` on Debian/Ubuntu, `@development-tools` on Fedora) — BoringSSL compiles roughly 6–8 minutes from cold; subsequent builds cache.
- **libclang resource headers** — `boring-sys`'s bindgen needs `stddef.h` from clang's resource dir. Ubuntu 24.04 ships `libclang.so.1` without the resource headers, so the workspace's `.cargo/config.toml` points `BINDGEN_EXTRA_CLANG_ARGS` at `/usr/lib/gcc/x86_64-linux-gnu/13/include -I/usr/include`. Distributions that install the full `clang` package (Fedora, Arch) don't need the workaround; the env var is harmless there.
- **~20 GB scratch disk** during the first build. BoringSSL + quiche + release artifacts together hit ~12 GB; tests in debug profile double that.

### Cross-compile notes

Cross-compiling to `aarch64-unknown-linux-gnu` or `x86_64-unknown-linux-musl` works from a stable host at the workspace MSRV (**1.88**) **provided the cross cmake + C toolchain are installed**. The XDP ebpf crate is outside the workspace members list and is not built by `cargo build --workspace` — it has its own pinned nightly toolchain per ADR `docs/decisions/ebpf-toolchain-separation.md`.

## Kernel floor

- **Linux 5.1+** for `io_uring`. Verified at process start: `lb_io::Runtime::new()` logs `backend=io_uring` or `backend=epoll`. Older kernels silently fall back.
- **Linux ≥ 5.15** (effective floor) for the XDP data plane. The aya-ebpf program uses neither BPF ring buffer nor kfuncs, and its BTF/CO-RE relocations load cleanly, so the effective verifier floor is ~5.15 LTS; 5.15 LTS / 6.1 LTS / 6.6 are the officially-validated LTS window (see `audit/ebpf/verifier-logs/`). The XDP data plane has additionally been **validated live on Linux 7.0** (audit foundation pass D-1: native ENA `xdpdrv` attach on `ens5`, full data path + state restore — see `audit/foundation-pass/d1-native-attach-result.md` and `audit/ebpf/verifier-logs/7.0.log.committed`). **7.x is NOT YET in the official verifier-baseline matrix**; whether to formally extend the supported/validated matrix to 7.x is an open R7 product decision recorded in `audit/ebpf/verifier-logs/README.md` (not gate-blocking; 7.0 itself is verified working).
- **glibc ≥ 2.31** (or the musl static build).

## systemd unit (minimal)

`/etc/systemd/system/expressgateway.service`:

```ini
[Unit]
Description=ExpressGateway L4/L7 load balancer
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=expressgateway
Group=expressgateway
ExecStart=/usr/local/bin/expressgateway /etc/expressgateway/lb.toml
ExecReload=/bin/kill -HUP $MAINPID
KillMode=mixed
KillSignal=SIGTERM
TimeoutStopSec=30

# Linux capabilities
AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_BPF
CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_BPF

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
RestrictSUIDSGID=true
RestrictRealtime=true
LockPersonality=true
MemoryDenyWriteExecute=true
ReadOnlyPaths=/
ReadWritePaths=/var/log/expressgateway /run/expressgateway
DevicePolicy=closed

# Resource limits
LimitNOFILE=1048576
LimitMEMLOCK=infinity

# Restart policy
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

### Capability rationale

- **`CAP_NET_BIND_SERVICE`** — bind to ports < 1024 (443 for HTTPS) as a non-root user.
- **`CAP_NET_ADMIN`** — `SO_REUSEPORT`, raw socket-options the kernel
  hides from unprivileged users, and netlink for the XDP userspace
  loader. Required for any XDP attach.
- **`CAP_BPF`** (Linux ≥ 5.8) — load + attach the XDP program. On
  pre-5.8 kernels the loader transparently falls back to checking for
  `CAP_SYS_ADMIN` (see SEC-2-11 in `audit/security/round-2-review.md`).

**Effective capability matrix**:

| Kernel       | Required (XDP off)        | Required (XDP on)                                      |
|--------------|---------------------------|--------------------------------------------------------|
| Linux ≥ 5.8  | `CAP_NET_BIND_SERVICE`    | `CAP_NET_BIND_SERVICE`, `CAP_NET_ADMIN`, `CAP_BPF`     |
| Linux < 5.8  | `CAP_NET_BIND_SERVICE`    | `CAP_NET_BIND_SERVICE`, `CAP_NET_ADMIN`, `CAP_SYS_ADMIN` |

The capability check landed with SEC-2-11 (`5064a11`,
`e44117d`). The userspace loader at startup logs which path it took
and refuses to attach if neither shape is present.

## Sysctls (recommended)

```ini
# /etc/sysctl.d/90-expressgateway.conf
net.core.somaxconn = 65535
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.core.netdev_max_backlog = 30000
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.tcp_fin_timeout = 15
net.ipv4.tcp_tw_reuse = 1
net.ipv4.ip_local_port_range = 2000 65500

# For XDP conntrack scale (Pillar 4b)
net.netfilter.nf_conntrack_max = 1048576

# Kernel allows pools of bound sockets
net.ipv4.tcp_keepalive_time = 60
net.ipv4.tcp_keepalive_intvl = 10
net.ipv4.tcp_keepalive_probes = 6
```

PROMPT.md §7 Backpressure lists the listener socket options the binary sets through `lb_io::sockopts::apply_listener`: `SO_REUSEADDR`, `SO_REUSEPORT`, `TCP_NODELAY`, `SO_KEEPALIVE`, `SO_RCVBUF=262144`, `SO_SNDBUF=262144`, `SO_BACKLOG=50000`. The sysctl values above give the kernel headroom to honor those requests.

## rlimits

- **`nofile = 1_048_576`**: the pool (`per_peer_max=8`, `total_max=256` by default) plus client-side listener accept rate can easily consume tens of thousands of file descriptors.
- **`memlock = infinity`**: required for XDP map pinning and `io_uring`'s registered buffers (Pillar 1c future).

## User

Create a dedicated service user: `useradd --system --shell /usr/sbin/nologin --home-dir /var/lib/expressgateway expressgateway`. No shell, no home.

## TLS material

Certificate and key paths go in the TOML; see `CONFIG.md`. Rotation strategy:

1. Write new `cert.pem` and `key.pem` into `/etc/expressgateway/tls/` atomically (rename, don't truncate). The private key must be `0600` (group/other-readable keys are rejected at load).
2. Send **SIGUSR1** (`kill -USR1 <pid>`) — this is the **TLS cert** reload path. (SIGHUP reloads the *config TOML*, not cert files; see "Signals" below.)
3. The in-process `TicketRotator` (`crates/lb-security/src/ticket.rs`) keeps the previous ticket key valid for its `overlap` window so sessions encrypted before the reload continue to decrypt.

### Signals

| Signal | Action |
|--------|--------|
| **SIGHUP** | Re-read the config TOML; validate-first; apply the swappable subset (backends, HTTP timeouts) live; log + skip restart-required changes; roll back atomically on invalid config. `systemctl reload` (`ExecReload=/bin/kill -HUP $MAINPID`). |
| **SIGUSR1** | Re-read TLS cert/key files and atomically swap the `TlsConfigBundle` under in-flight handshakes (REL-2-03). |
| **SIGTERM** | Graceful drain (lameduck `/readyz` → settle → cancel → bounded budget). `systemctl stop`. |
| **SIGINT** | Same drain path (interactive). |

TLS listeners are wired through `tokio_rustls::TlsAcceptor`
(`crates/lb/src/main.rs`). REL-2-03 closed the hot-reload path:
every TLS listener holds an `Arc<ArcSwap<TlsConfigBundle>>` (see
`crates/lb-security/src/ticket.rs` `SharedTlsBundle`), and SIGUSR1
atomically swaps in a freshly-built bundle. On parse/validate
failure the old bundle stays live. See
`audit/reliability/round-2-review.md` REL-2-03 for the audit
closure.

## XDP toolchain & the shipped BPF ELF

`crates/lb-l4-xdp/ebpf/src/main.rs` is a real aya-ebpf XDP program, and the
**compiled BPF ELF ships in-tree** at `crates/lb-l4-xdp/src/lb_xdp.bin`. The
loader's `build.rs` picks it up via `#[cfg(lb_xdp_elf)]`, so a stock
`cargo build --release` produces a binary whose XDP loader can `kernel_load`
+ `attach` the committed object. The data path has been **validated live on
Linux 7.0** (native ENA `xdpdrv` attach on `ens5`, full data path + state
restore — `audit/foundation-pass/d1-native-attach-result.md`).

XDP is **off by default** (`[runtime].xdp_enabled = false`); when disabled,
L4 traffic goes through the kernel TCP/UDP stack normally with no loss.

### Rebuilding the ELF from source (optional)

You only need this if you change the eBPF program. Building the BPF ELF
requires:

- `bpf-linker` (cargo subcommand): `cargo install bpf-linker --locked`.
- LLVM-18 development headers (`llvm-18-dev` on Debian/Ubuntu).
- A nightly rustc with the `rust-src` component.

The ebpf crate has its own pinned nightly toolchain (separate from the
workspace MSRV 1.88) per `docs/decisions/ebpf-toolchain-separation.md`.
Build it on a runner with that toolchain and commit the resulting `.bin`;
`scripts/build-xdp.sh` drives the build, and the loader's CODE-2-07 size
asserts pin the Rust↔ELF map layout so a stale `.bin` fails closed.

## ENA native-XDP requirements (hard deployment constraint)

The shipped `lb_xdp.bin` is built **without XDP multi-buffer (frags)
support**. On the AWS `ena` driver, native (`xdpdrv` / `XDP_FLAGS_DRV_MODE`)
attach is **refused by the driver** unless BOTH of the following hold on
the target interface:

1. **MTU ≤ 3498.** With a larger MTU (e.g. the VPC jumbo default
   `9001`) the `ena` driver rejects native XDP with
   `the current MTU (<n>) is larger than the maximum allowed MTU
   (3498) while xdp is on`. This is a direct consequence of the
   no-frags build — a frags-enabled object would lift this.
2. **Combined channels ≤ max/2** (`ethtool -L <iface> combined
   <≤max/2>`). The `ena` driver reserves a dedicated XDP TX queue per
   channel; at `combined == max` native attach fails with
   `the Rx/Tx channel count should be at most half of the maximum
   allowed channel count`.

Operational guidance for a native-XDP deployment on ENA:

- Set `MTU ≤ 3498` on the data-plane interface, **or** do not enable
  native XDP (the loader falls back to `skb`/generic with a significant
  performance penalty — see RUNBOOK `LbXdpAttachMode`).
- Set `ethtool -L <iface> combined <≤ half of max>` before attach.
- These are verified by the privileged D-1 test
  `lb-l4-xdp/tests/xdp_attach_mode.rs::d1_native_attach::
  drv_mode_attach_to_ens5_proves_live_datapath`, which transiently
  applies them for the attach window only and restores them on
  teardown.

**Known follow-up (not yet done):** rebuild the eBPF object with XDP
multi-buffer / frags support so native XDP works at the production
jumbo-frame MTU without lowering it. Tracked in
`audit/round-9/d1-native-xdp-constraint.md`.

## Observability

- **Logs**: `RUST_LOG=info` at start. Default formatter is plain text;
  the JSON formatter (`tracing_subscriber::fmt::format::json`) is
  available behind a config flag — see REL-2-06 in the audit for
  current status. Route with journald: `journalctl -u expressgateway -f`.
- **Metrics**: Prometheus text exposition at the
  `[observability].metrics_bind` address (default `127.0.0.1:9090`).
  `GET /metrics` is `text/plain; version=0.0.4`; `GET /healthz` returns
  200; `GET /readyz` flips to 503 during drain (lameduck signal).
  Full family catalog is `METRICS.md`.
- **Health**: in addition to the admin endpoints above, the binary
  exits non-zero on config parse/validate errors and on any
  un-recovered main-loop `anyhow::Error`. systemd's `Restart=on-failure`
  brings it back.

## Verification after deploy

1. `systemctl status expressgateway` → `active (running)`.
2. `journalctl -u expressgateway -n 20` → `lb-io runtime ready backend=io_uring` (or `epoll`), `TCP backend pool ready`, `DNS resolver ready`, one `listening on ...` line per listener.
3. `ss -tln` → one socket per listener address in LISTEN state.
4. Send a test request; verify upstream receives it; verify the connection count on the backend increments.
5. `systemctl reload expressgateway` (SIGHUP); watch logs for the
   `SIGHUP config reload pass complete` line (and the absence of
   `rolling back` / `rejected`), and verify no in-flight connections
   dropped. A swappable change (e.g. an edited backend pool) logs the
   per-field `SIGHUP: …` diff and applies live; a restart-required
   change (e.g. `protocol`/`tls`) is logged and skipped, not applied.
   The `reload_under_traffic` and `tests/reload_zero_drop.rs`
   integration tests exercise the validate-first / atomic-swap /
   honesty-contract path. To change a restart-required field, edit the
   TOML and restart the unit.

See `RUNBOOK.md` for ongoing operations.

## CI conformance (optional)

### h2spec — HTTP/2 conformance for `h1s` listeners

`tests/h2spec.rs` spawns the gateway's H2s listener on an ephemeral port
and invokes the [`h2spec`](https://github.com/summerwind/h2spec) binary.
The test passes green if `h2spec` is absent from `$PATH` (it logs
`h2spec not installed; skipping`), so the binary is **optional for local
dev** but **required for strict CI conformance**.

Install on Linux:

```
curl -L https://github.com/summerwind/h2spec/releases/download/v2.6.0/h2spec_linux_amd64.tar.gz \
    | tar -xz
sudo install -m 0755 h2spec /usr/local/bin/
h2spec --version
```

On macOS/BSD, download the matching archive from the same release page.
Once installed, `cargo test --test h2spec` exercises the full spec
suite (`-t` for TLS, `-k` to accept the self-signed cert).
