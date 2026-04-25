# Runbook

Operational procedures for ExpressGateway. Assume `systemd` is the process supervisor (see `DEPLOYMENT.md`).

## Startup

1. `systemctl start expressgateway`
2. Confirm with `systemctl status expressgateway`.
3. Look for these log lines (order matters — a missing line signals a config or environment problem):
   - `ExpressGateway v<version>`
   - `configuration loaded from <path>` with `listeners=N`
   - `lb-io runtime ready backend=io_uring|epoll high_water=65536 low_water=32768`
   - `TCP backend pool ready (defaults from PROMPT.md §21)`
   - `DNS resolver ready (positive cap 300s, negative TTL 5s)`
   - one `listener ...` line per entry in `[[listeners]]`

If you see `listener has no backends configured — skipping`, the listener was dropped; inspect the TOML.

## Drain (graceful shutdown)

```
systemctl stop expressgateway
```

This sends SIGTERM. The binary's signal handler (`crates/lb/src/main.rs`) triggers `tokio::signal::ctrl_c().await` and exits the main select. `KillMode=mixed` in the unit file means the main process receives SIGTERM and spawned tasks get SIGKILL after the `TimeoutStopSec=30` grace.

Connections currently in `copy_bidirectional` are allowed to complete until the timeout. The binary does not yet send `Connection: close` on active H1/H2 streams during drain — that is a Pillar 3b refinement.

For a zero-drop deploy on a TCP-only workload, use `SO_REUSEPORT` + staged rollout: start the new binary on the same address, wait for the pool to warm, then stop the old binary. `reload_zero_drop` in `tests/` exercises this pattern in-process.

## Configuration reload (SIGHUP)

```
systemctl reload expressgateway
```

`ExecReload=/bin/kill -HUP $MAINPID` dispatches SIGHUP. The control plane in `crates/lb-controlplane` re-reads the TOML, validates, and swaps the live config atomically via `ArcSwap`. On validation failure the old config stays active and the error is logged.

What reloads atomically:
- Backend list per listener (added / removed / reweighted).
- DNS-resolved backend addresses (hostnames re-resolved via `DnsResolver::resolve`).
- Future: detector thresholds, ticket rotator interval (Pillar 3b).

What requires a restart:
- Listener address/port changes — the socket is bound at startup.
- Kernel I/O backend switch (`io_uring` ↔ `epoll`).
- TLS cert swap at the listener level (see the next section for the ticket-key version).

## TLS certificate rotation

Once Pillar 3b wires TLS into the binary:

1. Write new PEM files into `/etc/expressgateway/tls/` with a unique filename (e.g. `cert.20260423.pem`).
2. Update the TOML `[[tls.sni_certs]]` entry `cert_path` + `key_path` (optionally by symlink flip).
3. `systemctl reload expressgateway` — the swap is atomic via `ArcSwap<TlsStore>`.

The in-process **ticket-key rotator** (`crates/lb-security/src/ticket.rs::TicketRotator`) rotates the TLS resumption key daily by default with a 24 h overlap window. Encrypted tickets from the previous key continue to decrypt for that overlap, so reloads don't invalidate active TLS sessions.

Emergency ticket-key purge (e.g. suspected key compromise):

- The rotator's public surface exposes `rotate_if_due`; an emergency rotate endpoint is not yet shipped. Workaround: set `rotation_interval` to 1 minute in the TOML, SIGHUP, wait 90 s, restore to daily.

## Config rollback

If a SIGHUP applied a bad config and the service is misbehaving but still accepting connections:

1. `git -C /etc/expressgateway/ log --oneline -5` (if the config is in git) → identify previous good commit.
2. `git -C /etc/expressgateway/ checkout <sha> -- lb.toml`.
3. `systemctl reload expressgateway`.

If the service is stuck or crashing:

1. `systemctl stop expressgateway`.
2. Restore a known-good config.
3. `systemctl start expressgateway`.

## Reading logs

Default log level is `info`. Override at startup via `Environment=RUST_LOG=debug` in the unit file, or at runtime by restarting with the env set.

Useful filters:

- `journalctl -u expressgateway --since '5 min ago'`
- `journalctl -u expressgateway -p err` — errors and worse.
- `journalctl -u expressgateway -g 'backend pool'` — pool lifecycle.
- `journalctl -u expressgateway -g 'liveness probe'` — Pingora EC-01 probe fires.
- `journalctl -u expressgateway -g 'dns'` — resolver hits / misses / refreshes.

Structured-field filtering (logs are plain text today; JSON is a `tracing-subscriber::fmt::format::json` flip in `crates/lb/src/main.rs` away):

```
journalctl -u expressgateway -o json | jq 'select(.MESSAGE | test("dns"))'
```

## XDP diagnosis

Pillar 4b wires the loader into the binary; until then, this section describes what to check once XDP load is attempted.

Preconditions:
- `lb_xdp.bin` exists (the compiled BPF ELF — see `DEPLOYMENT.md` XDP toolchain caveat).
- Process has `CAP_BPF` and `CAP_NET_ADMIN`.
- `sudo` not required if caps are granted.

Inspect loaded programs:
```
sudo bpftool prog show | grep xdp_lb
sudo bpftool prog dump xlated id <id>
sudo bpftool map show | grep -E 'CONNTRACK|L7_PORTS|ACL_DENY|STATS'
```

Check an XDP program is attached to an interface:
```
ip link show dev <iface>
# Look for 'xdp', 'xdpgeneric', or 'xdpdrv' on the link
```

Detach manually (if the binary died without cleanup):
```
sudo ip link set dev <iface> xdp off
```

Common errors from the loader (`crates/lb-l4-xdp/src/loader.rs::XdpLoaderError`):
- `InvalidElf` — the bundled `lb_xdp.bin` is corrupt or was built for the wrong target. Rebuild.
- `Parse(_)` — aya failed to parse the ELF. Check LLVM version used to build.
- `Attach(_)` — verifier rejected the program or interface doesn't support the requested mode. Retry with `XdpMode::Skb` (generic XDP) to isolate.
- `MapMissing(_)` — map name mismatch between ELF and userspace expectations. The ELF was built from a different source than the loader expects.

Verifier log capture:
```
sudo bpftool prog load crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/lb_xdp 2>&1
```

## DNS cache inspection

The resolver keeps its cache in-memory (`crates/lb-io/src/dns.rs::DnsResolver`). There is no external inspect endpoint today. Debug methods on the struct (`cache_size`, `refresh_all`) are used by tests.

To trigger a manual refresh of all entries, restart or SIGHUP — the refresh-interval loop re-queries naturally, and SIGHUP re-reads the backend list which triggers resolves.

## Pool diagnostics

The pool struct (`crates/lb-io/src/pool.rs::TcpPool`) exposes `idle_count()`. No admin endpoint today.

If you see elevated new-connection rates to backends:
- Check `per_peer_max` — a low value starves reuse.
- Check `idle_timeout` / `max_age` — values shorter than request inter-arrival times force churn.
- Check the liveness probe: peers behind buggy middleboxes that send FIN early will discard pooled conns. Grep logs for `pool probe discarded`.

## Panics

The Cloudflare 2025 panic-free rule means library code never panics. If you see a panic in logs, it is in the binary (`crates/lb/src/main.rs`) and indicates one of:

- An `anyhow::bail!` or `?` on a `Result<..>` — check the preceding log line for the error `Context`.
- Tokio runtime poisoning — restart.

File an issue with the panic stack trace and the last 200 lines of log.

## Version check

```
lb --version     # not implemented; run `cargo pkgid` against the installed binary's source tree, or
strings /usr/local/bin/lb | grep ExpressGateway
```

The binary logs its version at startup; `journalctl -u expressgateway -r | head -20` finds the most recent startup and shows `ExpressGateway v<version>`.
