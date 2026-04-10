# ExpressGateway Control Plane Architecture

## Overview

The control plane provides runtime management of the load balancer via REST API
and gRPC services. It supports a single-node in-memory mode with HA scaffolding
traits for future distributed backends.

## Components

```
┌─────────────────────────────────────────────────────────┐
│                    Control Plane                         │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐ │
│  │   REST API   │  │  gRPC Server │  │  Config Store │ │
│  │   (axum)     │  │   (tonic)    │  │  (in-memory)  │ │
│  │  :9091       │  │  :50051      │  │               │ │
│  └──────┬───────┘  └──────┬───────┘  └───────┬───────┘ │
│         │                 │                   │         │
│  ┌──────┴─────────────────┴───────────────────┴──────┐ │
│  │                   Shared Store                     │ │
│  │  clusters: DashMap<String, ClusterConfig>          │ │
│  │  nodes: DashMap<String, NodeInfo>                  │ │
│  │  routes: DashMap<String, RouteConfig>              │ │
│  │  traffic_policies: DashMap<String, TrafficPolicy>  │ │
│  │  sessions: DashMap<String, NodeSession>            │ │
│  └────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
         ▲                    ▲
         │ REST               │ gRPC
    ┌────┴────┐         ┌─────┴─────┐
    │  Admin  │         │   Agent   │
    │  User   │         │   Node    │
    └─────────┘         └───────────┘
```

## REST API Endpoints

### Route Management
| Method | Path | Description |
|--------|------|-------------|
| GET | `/routes` | List all routes |
| POST | `/routes` | Create a route |
| GET | `/routes/:id` | Get route details |
| PUT | `/routes/:id` | Update a route |
| DELETE | `/routes/:id` | Delete a route |

### Cluster/Backend Management
| Method | Path | Description |
|--------|------|-------------|
| GET | `/clusters` | List all clusters |
| POST | `/clusters` | Create a cluster |
| GET | `/clusters/:id` | Get cluster details |
| PUT | `/clusters/:id` | Update a cluster |
| DELETE | `/clusters/:id` | Delete a cluster |
| POST | `/clusters/:id/nodes` | Register a backend node |
| DELETE | `/clusters/:id/nodes/:node` | Deregister a backend node |

### Health
| Method | Path | Description |
|--------|------|-------------|
| GET | `/clusters/:id/health` | Get per-node health status |
| PUT | `/clusters/:id/nodes/:node/health` | Override node health |

### Traffic Policies
| Method | Path | Description |
|--------|------|-------------|
| GET | `/clusters/:id/traffic-policy` | Get traffic policy |
| PUT | `/clusters/:id/traffic-policy` | Set full traffic policy |
| PUT | `/clusters/:id/traffic-policy/weights` | Update weights only |
| PUT | `/clusters/:id/traffic-policy/circuit-breaker` | Update circuit breaker |
| PUT | `/clusters/:id/traffic-policy/rate-limit` | Update rate limit |

### Metrics
| Method | Path | Description |
|--------|------|-------------|
| GET | `/metrics` | Prometheus text exposition format |

## gRPC Services

### NodeRegistrationService
- `Register(RegisterRequest) -> RegisterResponse` -- Node registration with auth token
- Reconnect protection via session validation
- Returns node ID and session token

### ConfigDistributionService
- `FetchConfig(ConfigRequest) -> ConfigResponse` -- Version-aware config fetch
- Delta detection: returns full config only when version differs
- Requires valid registration session

### NodeControlService
- `Drain(DrainRequest) -> DrainResponse` -- Graceful drain
- `Undrain(UndrainRequest) -> UndrainResponse` -- Resume traffic
- `Disconnect(DisconnectRequest) -> DisconnectResponse` -- Force disconnect
- State validation on all transitions

### HealthAggregationService
- `ReportHealth(HealthReport) -> HealthAck` -- Per-node health reports
- Aggregates health from all agent nodes

## Agent Node (`crates/agent/`)

State machine:
```
Disconnected → Connecting → Connected → Reconnecting → ShuttingDown
     ▲              │            │             │
     └──────────────┘            │             │
     (on failure)                └─────────────┘
                                 (on disconnect)
```

Features:
- Heartbeat at configurable interval (default 10s)
- Configuration sync with version tracking
- Health status reporting to control plane
- Exponential backoff reconnection (configurable: initial, max, multiplier, max_retries)
- Graceful shutdown via `tokio::sync::watch` channel

## HA Scaffolding

Three traits define the distributed coordination contract:

### ConfigStore
```rust
#[async_trait]
trait ConfigStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    async fn put(&self, key: &str, value: &[u8]) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn list(&self, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;
    async fn watch(&self, prefix: &str) -> Result<Box<dyn Stream<Item = ConfigEvent>>>;
    async fn cas(&self, key: &str, expected: Option<&[u8]>, value: &[u8]) -> Result<bool>;
}
```

### ServiceDiscovery
```rust
#[async_trait]
trait ServiceDiscovery: Send + Sync {
    async fn register(&self, service: &ServiceInfo) -> Result<()>;
    async fn deregister(&self, service_id: &str) -> Result<()>;
    async fn discover(&self, name: &str) -> Result<Vec<ServiceInfo>>;
    async fn watch(&self, name: &str) -> Result<Box<dyn Stream<Item = Vec<ServiceInfo>>>>;
    async fn health_update(&self, service_id: &str, healthy: bool) -> Result<()>;
}
```

### LeaderElection
```rust
#[async_trait]
trait LeaderElection: Send + Sync {
    async fn campaign(&self, name: &str) -> Result<LeaderGuard>;
    async fn is_leader(&self) -> bool;
    async fn leader_info(&self) -> Result<Option<NodeInfo>>;
    async fn resign(&self) -> Result<()>;
}
```

### Implementations

| Backend | Status | Wire Format |
|---------|--------|-------------|
| In-Memory | Fully functional | DashMap + broadcast channels |
| etcd | Stub (`todo!()`) | gRPC, KV API v3, watch API, election API |
| Consul | Stub (`todo!()`) | HTTP API, KV store, service catalog, session-based locking |
| ZooKeeper | Stub (`todo!()`) | Binary protocol, znodes, ephemeral nodes, watches |

## Configuration

### File-Based (TOML)
- Parsed via `serde` + `toml` crate
- Validated on load (unknown fields rejected, value ranges checked)
- Per-listener I/O backend override support

### Hot-Reload
- File watcher via `notify` crate (inotify on Linux)
- Watches parent directory for atomic renames (certbot pattern)
- Debounced to prevent reload storms
- SIGHUP signal also triggers reload
- Lock-free config swap via `arc-swap::ArcSwap`
- Reload failures are logged but do not crash the process

### HA Distributed Config
- Uses `ConfigStore` trait (scaffolding only)
- `watch()` returns a stream of `ConfigEvent` (Put/Delete)
- `cas()` provides compare-and-swap for safe concurrent updates
