# Deployment Models

## Single-Node Deployment

The simplest deployment: one ExpressGateway instance handles all traffic.

```
                    Internet
                       │
                ┌──────▼──────┐
                │ ExpressGW   │
                │             │
                │ ┌─────────┐ │
                │ │ XDP     │ │  ← Optional L4 fast path
                │ │ (NIC)   │ │
                │ └────┬────┘ │
                │      │      │
                │ ┌────▼────┐ │
                │ │ Data    │ │  ← L4/L7 proxy engine
                │ │ Plane   │ │
                │ └────┬────┘ │
                │      │      │
                │ ┌────▼────┐ │
                │ │ Control │ │  ← REST API + gRPC
                │ │ Plane   │ │
                │ └─────────┘ │
                └──────┬──────┘
                       │
            ┌──────────┼──────────┐
            ▼          ▼          ▼
        Backend 1  Backend 2  Backend N
```

### Configuration

```toml
[global]
environment = "production"

[runtime]
backend = "auto"
worker_threads = 0        # auto-detect CPUs
xdp_enabled = true
xdp_interface = "eth0"

[[listeners]]
name = "https"
protocol = "http"
bind = "0.0.0.0:443"

[tls]
enabled = true
[tls.server]
default_cert = "/etc/expressgateway/cert.pem"
default_key  = "/etc/expressgateway/key.pem"

[[clusters]]
name = "web-backend"
lb_strategy = "least_connection"

[[clusters.nodes]]
address = "10.0.1.10:8080"
[[clusters.nodes]]
address = "10.0.1.11:8080"
[[clusters.nodes]]
address = "10.0.1.12:8080"

[[routes]]
path = "/"
cluster = "web-backend"

[controlplane]
enabled = true
rest_bind = "127.0.0.1:9091"    # admin-only
grpc_bind = "127.0.0.1:50051"
```

### Systemd Unit

```ini
[Unit]
Description=ExpressGateway Load Balancer
After=network.target

[Service]
Type=notify
ExecStart=/usr/local/bin/expressgateway --config /etc/expressgateway/config.toml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
LimitNOFILE=1000000
LimitMEMLOCK=infinity
AmbientCapabilities=CAP_NET_BIND_SERVICE CAP_BPF CAP_NET_ADMIN
NoNewPrivileges=true

[Install]
WantedBy=multi-user.target
```

### Required Capabilities
- `CAP_NET_BIND_SERVICE` — bind to ports < 1024
- `CAP_BPF` — load eBPF programs (XDP)
- `CAP_NET_ADMIN` — attach XDP programs, set socket options

### Kernel Tuning

```bash
# /etc/sysctl.d/expressgateway.conf
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535
net.core.netdev_max_backlog = 65535
net.ipv4.tcp_fastopen = 3
net.ipv4.tcp_tw_reuse = 1
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_rmem = 4096 87380 16777216
net.ipv4.tcp_wmem = 4096 65536 16777216
net.ipv4.ip_local_port_range = 1024 65535
```

## Multi-Node Deployment (HA Scaffolding)

Multiple ExpressGateway instances behind an anycast VIP or DNS round-robin.

```
                    Internet
                       │
              ┌────────▼────────┐
              │  Anycast VIP    │
              │  or DNS RR      │
              └───┬────┬────┬───┘
                  │    │    │
        ┌─────────▼┐ ┌▼────┴────┐ ┌──────────┐
        │ EG Node 1│ │ EG Node 2│ │ EG Node N│
        │ (leader) │ │(follower)│ │(follower)│
        └─────┬────┘ └────┬─────┘ └────┬─────┘
              │            │             │
              └──────┬─────┴─────────────┘
                     │
              ┌──────▼──────┐
              │ Config Store│  ← etcd / Consul / ZooKeeper
              │  (future)   │     (stub implementations)
              └─────────────┘
```

### Current State

The HA scaffolding provides trait definitions and in-memory implementations:

| Component | Trait | In-Memory | etcd | Consul | ZooKeeper |
|-----------|-------|-----------|------|--------|-----------|
| Config Store | `ConfigStore` | Functional | Stub | Stub | Stub |
| Service Discovery | `ServiceDiscovery` | Functional | Stub | Stub | Stub |
| Leader Election | `LeaderElection` | Functional | Stub | Stub | Stub |

### Multi-Node with In-Memory (Independent Nodes)

Each node operates independently with its own config:

```toml
# Node 1
[controlplane]
enabled = true
grpc_bind = "0.0.0.0:50051"

# Node 2 - identical config
[controlplane]
enabled = true
grpc_bind = "0.0.0.0:50051"
```

State is NOT synchronized between nodes. Each node:
- Maintains its own backend list and health state
- Runs independent health checks
- Makes independent LB decisions

This is suitable for stateless workloads where backend lists are identical.

### Future: Distributed HA

When HA backends are implemented, nodes will:
1. Elect a leader via `LeaderElection`
2. Synchronize config via `ConfigStore`
3. Register themselves via `ServiceDiscovery`
4. Leader handles config writes; followers forward to leader
5. Health state aggregated across nodes

### Wire Format Expectations

#### etcd
- Transport: gRPC (HTTP/2)
- KV API v3: `/etcdserverpb.KV/{Range,Put,DeleteRange}`
- Watch API: `/etcdserverpb.Watch/Watch` (bidirectional stream)
- Election: `/v3election.Election/{Campaign,Resign,Leader}`
- Lease-based TTL for ephemeral state

#### Consul
- Transport: HTTP/1.1 REST
- KV: `GET/PUT/DELETE /v1/kv/:key`
- Service catalog: `PUT /v1/agent/service/register`
- Health: `GET /v1/health/service/:name`
- Session-based locking: `PUT /v1/session/create`

#### ZooKeeper
- Transport: Binary protocol over TCP
- ZNode CRUD: create, get, set, delete
- Ephemeral nodes for service registration
- Sequential nodes for leader election
- Watches for change notification

## Resource Planning

### Connections per Instance

| Deployment Size | Connections | Workers | Memory | CPU |
|-----------------|-------------|---------|--------|-----|
| Small | 10K | 4 | 2 GB | 4 cores |
| Medium | 100K | 16 | 8 GB | 16 cores |
| Large | 1M | 32 | 32 GB | 32 cores |
| XL | 10M+ | 64+ | 64+ GB | 64+ cores |

### File Descriptor Limits

Rule of thumb: `max_fds = (max_connections * 2) + 1000`
- Each proxied connection uses 2 fds (client + backend)
- Plus: listeners, control plane, health checks, etc.

Set via systemd `LimitNOFILE` or `/etc/security/limits.conf`.

## Health Check Endpoint

```bash
# Application health
curl http://localhost:9091/health

# Prometheus metrics
curl http://localhost:9091/metrics

# Runtime config via REST API
curl http://localhost:9091/clusters
curl http://localhost:9091/routes
```
