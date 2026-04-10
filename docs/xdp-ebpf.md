# XDP/eBPF Fast Path Documentation

## Overview

ExpressGateway includes an XDP (eXpress Data Path) component for kernel-bypass
packet processing. This enables L4 load balancing, DDoS mitigation, and
connection steering at the NIC driver level before packets reach the kernel
networking stack.

## Architecture

```
                    NIC
                     │
            ┌────────▼────────┐
            │   XDP Program   │  ← Runs in NIC driver context
            │                 │
            │  ┌────────────┐ │
            │  │ Connection │ │  ← BPF HashMap
            │  │   Table    │ │
            │  └────────────┘ │
            │  ┌────────────┐ │
            │  │  Backend   │ │  ← BPF Array
            │  │   List     │ │
            │  └────────────┘ │
            │  ┌────────────┐ │
            │  │    ACL     │ │  ← BPF LPM Trie
            │  │  (deny)    │ │
            │  └────────────┘ │
            └────────┬────────┘
                     │
          ┌──────────┼──────────┐
          ▼          ▼          ▼
     XDP_PASS   XDP_DROP   XDP_TX/XDP_REDIRECT
     (to stack) (DDoS      (DSR: direct
                 filter)    server return)
```

## XDP Modes

| Mode | Flag | Description | Performance |
|------|------|-------------|-------------|
| Native (DRV) | `XDP_DRV` | Runs in NIC driver | Highest (10M+ pps) |
| Offload | `XDP_OFFLOAD` | Runs on NIC hardware | Highest (NIC-limited) |
| Generic (SKB) | `XDP_SKB` | Runs in kernel network stack | Moderate (fallback) |

Mode selection priority: `XDP_DRV` → `XDP_OFFLOAD` → `XDP_SKB`

## BPF Maps

### Connection Table (`BPF_MAP_TYPE_HASH`)
- Key: `(src_ip, src_port, dst_ip, dst_port, protocol)` — 5-tuple
- Value: `(backend_ip, backend_port, timestamp, flags)`
- Used for: Connection affinity, session tracking
- Shared with userspace for state synchronization

### Backend List (`BPF_MAP_TYPE_ARRAY`)
- Index: Backend ID
- Value: `(ip, port, weight, health_status)`
- Updated by userspace health checker

### ACL Table (`BPF_MAP_TYPE_LPM_TRIE`)
- Key: IP prefix (IPv4/IPv6 with prefix length)
- Value: Action (allow/deny)
- Used for: IP-based access control at wire speed

### Stats (`BPF_MAP_TYPE_PERCPU_ARRAY`)
- Per-CPU counters: packets processed, bytes, drops, errors
- Aggregated by userspace via `XdpStats::merge()`

## L4 Load Balancing in XDP

### Direct Server Return (DSR)
When possible, the XDP program modifies packet headers to forward directly to
the backend without proxying through userspace:

1. Client sends packet to VIP
2. XDP program intercepts at NIC driver
3. Consistent hash (Maglev) selects backend
4. Rewrite destination MAC/IP to backend
5. `XDP_TX` or `XDP_REDIRECT` sends packet out

Return traffic goes directly from backend to client (DSR).

### Consistent Hashing
- **Maglev** algorithm for backend selection
- Minimal disruption on backend addition/removal
- Deterministic: same 5-tuple → same backend

### Fallback to Userspace
Packets are passed to the kernel stack (`XDP_PASS`) when:
- TLS termination is required (packet inspection needed)
- L7 protocol handling required (HTTP, gRPC, WebSocket)
- Connection requires stateful inspection
- Port is marked as L7 via `add_l7_port()`

## DDoS Mitigation

### SYN Flood Protection
- SYN cookie validation in XDP
- Rate limiting per source IP
- Blackholing persistent offenders via ACL map

### Packet Rate Limiting
- Global PPS limit (configurable, default 1M)
- Per-IP PPS limit with burst allowance
- Actions: `XDP_DROP` (silent), connection reset, tarpit

## Userspace Integration (aya)

The userspace component uses the `aya` crate (pure Rust):

```rust
// Program lifecycle
let manager = XdpManager::new(config)?;
manager.load_and_attach("eth0", XdpMode::Driver)?;

// Backend management
manager.update_backend(0, backend_info)?;
manager.remove_backend(0)?;

// ACL management
manager.add_acl_entry(AclKey::from_ipv4(ip, prefix_len), AclAction::Deny)?;

// L7 port management
manager.add_l7_port(443)?;     // TLS → pass to userspace
manager.remove_l7_port(443)?;  // Remove L7 override

// Stats
let stats = manager.get_stats()?;
println!("Total packets: {}", stats.total_packets());

// Cleanup
manager.detach()?;
```

## Session Cleanup

Background task sweeps expired connection table entries:
- Configurable sweep interval
- `sweep_count()` returns count without allocation
- Graceful shutdown via `mpsc::Sender` signal
- Returns `(JoinHandle, mpsc::Sender)` for lifecycle management

## Error Handling

`XdpError` enum with 8 variants:
- `PlatformUnavailable` — not on Linux / no XDP support
- `AlreadyAttached` — program already loaded
- `NotAttached` — operation requires attached program
- `LoadFailed` — eBPF program load error
- `AttachFailed` — XDP attach error
- `DetachFailed` — XDP detach error
- `MapError` — BPF map operation error
- `ModeUnsupported` — requested XDP mode not available
- `KernelVersion` — kernel too old

## Kernel Requirements

| Feature | Minimum Kernel |
|---------|---------------|
| XDP basic (SKB) | 4.8 |
| XDP native (DRV) | 4.15 |
| BPF HashMap | 3.19 |
| BPF LPM Trie | 4.11 |
| BPF per-CPU array | 4.6 |
| XDP_REDIRECT | 4.14 |
| AF_XDP | 4.18 |

Recommended: Linux 5.11+ for full io_uring + XDP feature set.

## Configuration

```toml
[runtime]
xdp_enabled    = true
xdp_interface  = "eth0"
xdp_mode       = "driver"    # driver | offload | generic
```

## Loading Instructions

1. Build the eBPF program:
```bash
cargo xtask build-ebpf
```

2. Load via ExpressGateway config:
```toml
[runtime]
xdp_enabled = true
xdp_interface = "eth0"
```

3. Or load manually:
```bash
# Requires CAP_BPF + CAP_NET_ADMIN
sudo ./expressgateway --xdp-interface eth0 --xdp-mode driver
```

4. Verify attachment:
```bash
ip link show eth0 | grep xdp
bpftool prog list
bpftool map list
```

5. Detach:
```bash
ip link set eth0 xdp off
```
