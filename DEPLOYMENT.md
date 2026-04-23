# Deployment

This document covers running the `lb` binary on Linux in a production-adjacent shape. XDP-dependent items are flagged because they are currently source-only (see [XDP toolchain caveat](#xdp-toolchain-caveat)).

## Build

```
cargo build --release -p lb
```

Artifact: `target/release/lb`. Strip is enabled by the `[profile.release]` block in root `Cargo.toml`; LTO is set to `thin`.

### Build-time dependencies

- **`cmake`** (≥ 3.20) — required because `quiche` links BoringSSL and BoringSSL's build is driven by cmake. `cmake 3.28` confirmed to work on Ubuntu 24.04. See ADR `docs/decisions/quinn-to-quiche-migration.md`.
- **C/C++ toolchain** (`build-essential` on Debian/Ubuntu, `@development-tools` on Fedora) — BoringSSL compiles roughly 6–8 minutes from cold; subsequent builds cache.
- **libclang resource headers** — `boring-sys`'s bindgen needs `stddef.h` from clang's resource dir. Ubuntu 24.04 ships `libclang.so.1` without the resource headers, so the workspace's `.cargo/config.toml` points `BINDGEN_EXTRA_CLANG_ARGS` at `/usr/lib/gcc/x86_64-linux-gnu/13/include -I/usr/include`. Distributions that install the full `clang` package (Fedora, Arch) don't need the workaround; the env var is harmless there.
- **~20 GB scratch disk** during the first build. BoringSSL + quiche + release artifacts together hit ~12 GB; tests in debug profile double that.

### Cross-compile notes

Cross-compiling to `aarch64-unknown-linux-gnu` or `x86_64-unknown-linux-musl` works from a stable 1.85 host **provided the cross cmake + C toolchain are installed**. The XDP ebpf crate is outside the workspace members list and is not built by `cargo build --workspace` — it has its own pinned nightly toolchain per ADR `docs/decisions/ebpf-toolchain-separation.md`.

## Kernel floor

- **Linux 5.1+** for `io_uring`. Verified at process start: `lb_io::Runtime::new()` logs `backend=io_uring` or `backend=epoll`. Older kernels silently fall back.
- **Linux 5.15 LTS** or **6.1 LTS** for the XDP data plane (when Pillar 4b lands the loader integration). The aya-ebpf program's verifier constraints are tuned for those LTS kernels.
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
ExecStart=/usr/local/bin/lb /etc/expressgateway/lb.toml
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
- **`CAP_NET_ADMIN`** — `SO_REUSEPORT` and socket-option setting beyond what an unprivileged user can do; XDP userspace loader needs it for netlink.
- **`CAP_BPF`** — load + attach the XDP program when Pillar 4b wires it. Not strictly required for the current binary (XDP attach is not invoked), but granted so a SIGHUP that enables XDP does not require a restart.

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

1. Write new `cert.pem` and `key.pem` into `/etc/expressgateway/tls/` atomically (rename, don't truncate).
2. `systemctl reload expressgateway` (SIGHUP).
3. The in-process `TicketRotator` (`crates/lb-security/src/ticket.rs`) keeps the previous ticket key valid for its `overlap` window so sessions encrypted before the reload continue to decrypt.

TLS listener wiring is Pillar 3b; until then, the rotator is tested at the unit level only.

## XDP toolchain caveat

`crates/lb-l4-xdp/ebpf/src/main.rs` is a real aya-ebpf XDP program. Building the BPF ELF requires:

- `bpf-linker` (cargo subcommand): `cargo install bpf-linker --locked`.
- LLVM-18 development headers (`llvm-18-dev` on Debian/Ubuntu).
- A nightly rustc with the `rust-src` component.

**At the time of this writing `bpf-linker`'s transitive dependencies require rustc ≥ 1.88, but the project's MSRV is 1.85.** The install fails on our pinned toolchain. Options:

1. Bump the project MSRV to 1.88 (tracked as a Pillar 3b candidate; also drops `RUSTSEC-2026-0009`).
2. Pin older versions of the transitive deps (`cargo-platform`, `libloading`, `time`) via `cargo update --precise`.
3. Build the BPF ELF on a separate CI runner that has rustc 1.88 and commit the resulting `.bin` to `crates/lb-l4-xdp/src/lb_xdp.bin`; the loader's `build.rs` picks it up via `#[cfg(lb_xdp_elf)]`.

Until one of those lands, the XDP loader happy paths (`kernel_load`, `attach`) are not exercised. L4 proxying goes through the kernel TCP stack as normal; no traffic is lost.

## Observability

- **Logs**: `RUST_LOG=info` at start, JSON to stdout via `tracing_subscriber::fmt().with_env_filter(...)`. Route with journald: `journalctl -u expressgateway -f`.
- **Metrics**: not yet exposed via HTTP; see `METRICS.md`.
- **Health**: no external health endpoint. Use `ss -tln` or `curl -v 127.0.0.1:<port>` for a liveness smoke test; the binary exits non-zero on config parse / validate errors and on any main-loop `anyhow::Error`.

## Verification after deploy

1. `systemctl status expressgateway` → `active (running)`.
2. `journalctl -u expressgateway -n 20` → `lb-io runtime ready backend=io_uring` (or `epoll`), `TCP backend pool ready`, `DNS resolver ready`, one `listening on ...` line per listener.
3. `ss -tln` → one socket per listener address in LISTEN state.
4. Send a test request; verify upstream receives it; verify the connection count on the backend increments.
5. `systemctl reload expressgateway`; watch logs for `reload applied` and no errors; verify no in-flight connections dropped (covered by the internal `reload_zero_drop` integration test, but worth repeating in prod).

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
