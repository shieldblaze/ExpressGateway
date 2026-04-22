# PROMPT.md -- ShieldBlaze ExpressGateway: Full Rust Rewrite

> **Purpose**: This document is the master prompt for Claude Code to execute a complete rewrite of ShieldBlaze ExpressGateway from Java/Netty to Rust with XDP, io_uring, and epoll. It defines every feature to implement, the architecture, agent team structure, and quality standards. Every agent spawned during this project MUST read this file first.

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [Architecture Vision](#2-architecture-vision)
3. [Agent Team Structure](#3-agent-team-structure)
4. [Phase Execution Plan](#4-phase-execution-plan)
5. [Module 1: Foundation & Build System](#5-module-1-foundation--build-system)
6. [Module 2: XDP Data Plane (L4)](#6-module-2-xdp-data-plane-l4)
7. [Module 3: io_uring Async Runtime](#7-module-3-io_uring-async-runtime)
8. [Module 4: TLS & Cryptography](#8-module-4-tls--cryptography)
9. [Module 5: L4 Load Balancing & Session Persistence](#9-module-5-l4-load-balancing--session-persistence)
10. [Module 6: HTTP/1.1 Proxy](#10-module-6-http11-proxy)
11. [Module 7: HTTP/2 Proxy](#11-module-7-http2-proxy)
12. [Module 8: HTTP/3 & QUIC Proxy](#12-module-8-http3--quic-proxy)
13. [Module 9: gRPC Proxy](#13-module-9-grpc-proxy)
14. [Module 10: WebSocket Proxy](#14-module-10-websocket-proxy)
15. [Module 11: L7 Load Balancing & Routing](#15-module-11-l7-load-balancing--routing)
16. [Module 12: Health Checking & Circuit Breakers](#16-module-12-health-checking--circuit-breakers)
17. [Module 13: Security & DDoS Protection](#17-module-13-security--ddos-protection)
18. [Module 14: Observability & Metrics](#18-module-14-observability--metrics)
19. [Module 15: Configuration System](#19-module-15-configuration-system)
20. [Module 16: Control Plane](#20-module-16-control-plane)
21. [Module 17: Connection Management & Pooling](#21-module-17-connection-management--pooling)
22. [Module 18: Compression](#22-module-18-compression)
23. [Module 19: Proxy Protocol](#23-module-19-proxy-protocol)
24. [Module 20: Bootstrap & Lifecycle](#24-module-20-bootstrap--lifecycle)
25. [Testing Strategy](#25-testing-strategy)
26. [Performance Requirements](#26-performance-requirements)
27. [Quality Gates](#27-quality-gates)
28. [Reference: Complete Feature Parity Checklist](#28-reference-complete-feature-parity-checklist)

---

## 1. Project Overview

### What We Are Building

A complete rewrite of **ShieldBlaze ExpressGateway** -- a production-grade, high-performance L4/L7 load balancer -- from Java 25 / Netty 4.2 to **Rust** using:

- **XDP (eXpress Data Path)** for L4 (TCP/UDP) packet-level proxying at line rate via eBPF maps
- **io_uring** for L7 (HTTP/1.1, HTTP/2, HTTP/3, gRPC, WebSocket) proxying with kernel-bypassed async I/O
- **epoll** as a portable fallback when io_uring is unavailable (kernels < 5.1)

### Why Rust

- Zero-cost abstractions for the performance-critical data path
- Memory safety without garbage collection (no GC pauses under load)
- Native eBPF/XDP integration via `aya` (no C required)
- `io_uring` as a first-class async I/O backend
- Fearless concurrency with the ownership model
- Single static binary deployment

### Scope

**1:1 feature parity** with the existing Java implementation. Every protocol, algorithm, handler, security feature, and operational capability documented in this file MUST be implemented. No features may be dropped or deferred unless explicitly marked as `[DEFERRED]`.

### What Changed from Java

| Aspect | Java (Current) | Rust (Target) |
|--------|---------------|---------------|
| L4 data plane | Netty EventLoop (epoll/io_uring) | XDP/eBPF in-kernel forwarding |
| L7 data plane | Netty channel pipeline | io_uring async I/O (epoll fallback) |
| TLS | OpenSSL via netty-tcnative / BoringSSL | rustls (ring crypto backend) |
| HTTP/1.1 | Netty HttpCodec | hyper 1.x |
| HTTP/2 | Netty Http2FrameCodec | h2 crate |
| HTTP/3 / QUIC | Netty incubator-codec-quic | quinn + h3 |
| gRPC | Direct HTTP/2 frame handling | tonic (or raw h2 + prost) |
| Config backend | ZooKeeper / etcd / Consul | File-based (TOML) with hot-reload |
| Control plane | gRPC + REST (Spring WebFlux) | gRPC (tonic) + REST (axum) |
| Concurrency | JCTools / Agrona lock-free | crossbeam / std::sync::atomic |
| Memory | PooledByteBufAllocator | bytes crate + custom slab allocator |
| Build | Maven multi-module | Cargo workspace |

---

## 2. Architecture Vision

### Layer Architecture

```
+------------------------------------------------------------------+
|                        Control Plane (REST + gRPC)                |
|   Config Distribution | Node Registry | Health Monitoring         |
+------------------------------------------------------------------+
         |  config push (gRPC)        |  metrics pull
         v                            v
+------------------------------------------------------------------+
|                     Management Layer                              |
|   Config Hot-Reload | Lifecycle | Graceful Drain | Metrics Export |
+------------------------------------------------------------------+
         |                            |
         v                            v
+----------------------------+  +-----------------------------------+
|    XDP Data Plane (L4)     |  |     io_uring Data Plane (L7)      |
|                            |  |                                   |
|  BPF Maps:                 |  |  Protocol Handlers:               |
|   - TCP conn table         |  |   - HTTP/1.1 (hyper)              |
|   - UDP session table      |  |   - HTTP/2 (h2)                  |
|   - Backend selection      |  |   - HTTP/3 (quinn + h3)          |
|   - Rate limit counters    |  |   - gRPC (tonic / h2+prost)      |
|   - ACL trie               |  |   - WebSocket (tokio-tungstenite) |
|                            |  |                                   |
|  Actions:                  |  |  Services:                        |
|   - XDP_TX (hairpin)       |  |   - Connection pooling            |
|   - XDP_REDIRECT           |  |   - Load balancing                |
|   - XDP_PASS (to L7)       |  |   - TLS termination (rustls)      |
|   - XDP_DROP               |  |   - Compression                   |
|                            |  |   - Header manipulation           |
+----------------------------+  +-----------------------------------+
         |                            |
         v                            v
+------------------------------------------------------------------+
|                    Shared Subsystems                              |
|  Load Balancing Algorithms | Health Checks | Session Persistence  |
|  Circuit Breakers | Security (ACL/Rate Limit) | Metrics           |
+------------------------------------------------------------------+
```

### XDP vs io_uring Decision Matrix

| Traffic Type | Layer | Engine | Rationale |
|-------------|-------|--------|-----------|
| TCP proxy (passthrough, no TLS termination) | L4 | XDP | Pure packet forwarding, no payload inspection. BPF map lookup + header rewrite at NIC driver level. Line-rate performance. |
| UDP proxy (non-QUIC) | L4 | XDP | Stateless/session-mapped forwarding. BPF map for session affinity. No payload parsing needed. |
| TCP with TLS termination | L4+L7 | XDP_PASS -> io_uring | XDP handles ACL/rate-limit, then passes to userspace for TLS. |
| HTTP/1.1 | L7 | io_uring | Request parsing, header manipulation, routing, compression require userspace logic. |
| HTTP/2 | L7 | io_uring | Stream multiplexing, HPACK, flow control need full protocol state machine. |
| HTTP/3 / QUIC | L7 | io_uring | QUIC requires TLS 1.3, congestion control, stream management. Complex state machine. |
| gRPC | L7 | io_uring | Built on HTTP/2, needs protobuf awareness, deadline tracking. |
| WebSocket | L7 | io_uring | Upgrade handshake, frame parsing, ping/pong management. |

### Cargo Workspace Structure

```
expressgateway-rs/
├── Cargo.toml                    # Workspace root
├── PROMPT.md                     # This file
├── CLAUDE.md                     # Agent instructions
├── config/
│   └── default.toml              # Default configuration
├── crates/
│   ├── eg-core/                  # Core types, traits, errors
│   ├── eg-config/                # Configuration system (TOML, hot-reload)
│   ├── eg-xdp/                   # XDP/eBPF programs and userspace loader
│   │   ├── src/
│   │   │   ├── bpf/              # eBPF C or aya-bpf Rust programs
│   │   │   ├── maps.rs           # BPF map definitions
│   │   │   ├── loader.rs         # XDP program loader
│   │   │   └── lib.rs
│   │   └── Cargo.toml
│   ├── eg-runtime/               # io_uring / epoll async runtime abstraction
│   ├── eg-tls/                   # TLS (rustls), certificates, OCSP, mTLS
│   ├── eg-lb/                    # Load balancing algorithms (L4 + L7)
│   ├── eg-health/                # Health checking (TCP, UDP, HTTP)
│   ├── eg-pool/                  # Connection pooling (H1, H2, QUIC)
│   ├── eg-security/              # ACL, rate limiting, TLS fingerprinting
│   ├── eg-protocol-tcp/          # TCP L4 proxy handler
│   ├── eg-protocol-udp/          # UDP L4 proxy handler
│   ├── eg-protocol-http/         # HTTP/1.1 + HTTP/2 L7 proxy
│   ├── eg-protocol-h3/           # HTTP/3 + QUIC L7 proxy
│   ├── eg-protocol-grpc/         # gRPC proxy (over HTTP/2 and HTTP/3)
│   ├── eg-protocol-ws/           # WebSocket proxy
│   ├── eg-compression/           # Brotli, gzip, zstd, deflate
│   ├── eg-proxy-protocol/        # HAProxy PROXY protocol v1/v2
│   ├── eg-metrics/               # Prometheus metrics, access logging
│   ├── eg-controlplane/          # Control plane (gRPC + REST API)
│   ├── eg-agent/                 # Data plane agent (connects to CP)
│   └── eg-bootstrap/             # Main binary, lifecycle management
├── tests/
│   ├── integration/              # Cross-crate integration tests
│   └── e2e/                      # End-to-end tests
└── xtask/                        # Build tasks (XDP compilation, etc.)
```

---

## 3. Agent Team Structure

This project MUST be executed using Claude Code Agent Teams. The lead agent orchestrates work across specialized teammate agents. Each agent has deep domain expertise and specific responsibilities.

### Team Composition

#### Lead Agent: **Architect** (Opus mode)
- **Role**: System architect and team coordinator
- **Responsibilities**: Break work into tasks, assign to specialists, review integration points, resolve cross-cutting concerns, maintain architectural consistency
- **Model**: Opus (maximum reasoning capability)

#### Agent 1: **XDP Engineer** (Opus mode, worktree isolation)
- **Domain**: eBPF/XDP programs, BPF maps, packet processing, kernel bypass
- **Crates owned**: `eg-xdp`
- **Key skills**: aya-rs, eBPF verifier constraints, BPF map types (HashMap, LPM Trie, Array), XDP actions, packet parsing, checksum recalculation
- **Must verify**: All BPF programs pass the kernel verifier, no unbounded loops, proper bounds checking

#### Agent 2: **io_uring & Runtime Engineer** (Opus mode, worktree isolation)
- **Domain**: Async I/O, io_uring submission/completion rings, epoll fallback, event loops
- **Crates owned**: `eg-runtime`, `eg-core`
- **Key skills**: io-uring crate, tokio-uring, SQ/CQ ring management, buffer pools, multishot accept, fixed file descriptors
- **Must verify**: No blocking calls on async paths, proper CQE handling, buffer lifecycle correctness

#### Agent 3: **TLS & Cryptography Engineer** (Opus mode, worktree isolation)
- **Domain**: TLS 1.2/1.3, certificate management, OCSP, mTLS, SNI, ALPN
- **Crates owned**: `eg-tls`
- **Key skills**: rustls, webpki, rcgen, OCSP stapling, certificate hot-reload, CRL checking, cipher suite configuration
- **Must verify**: No TLS 1.1 in production, proper certificate chain validation, OCSP nonce verification, thread-safe context swaps

#### Agent 4: **HTTP Protocol Engineer** (Opus mode, worktree isolation)
- **Domain**: HTTP/1.1 (RFC 9110/9112), HTTP/2 (RFC 9113), protocol translation
- **Crates owned**: `eg-protocol-http`
- **Key skills**: hyper, h2, header manipulation, hop-by-hop stripping, chunked encoding, flow control, stream multiplexing, h2c, CONNECT tunneling, 100-continue, pipelining
- **Must verify**: RFC compliance for all protocol features, proper pseudo-header handling, flow control correctness, no request smuggling vectors

#### Agent 5: **QUIC & HTTP/3 Engineer** (Opus mode, worktree isolation)
- **Domain**: QUIC (RFC 9000), HTTP/3 (RFC 9114), 0-RTT, connection migration
- **Crates owned**: `eg-protocol-h3`
- **Key skills**: quinn, h3, QUIC connection pooling, Alt-Svc header injection, stateless retry, connection migration, NAT rebinding, path validation
- **Must verify**: TLS 1.3 mandatory for QUIC, proper connection ID routing, 0-RTT replay protection, flow control at connection and stream level

#### Agent 6: **gRPC & WebSocket Engineer** (Opus mode, worktree isolation)
- **Domain**: gRPC-over-HTTP/2, gRPC-over-HTTP/3, WebSocket (RFC 6455), WebSocket-over-HTTP/2 (RFC 8441)
- **Crates owned**: `eg-protocol-grpc`, `eg-protocol-ws`
- **Key skills**: tonic, prost, tokio-tungstenite, gRPC status mapping, deadline enforcement, streaming types (unary/server/client/bidi), WebSocket upgrade, frame handling, ping/pong, close handshake
- **Must verify**: Correct gRPC status code mapping (HTTP -> gRPC), deadline clamp at 300s, WebSocket frame size limits, bidirectional backpressure

#### Agent 7: **Load Balancing & Backend Engineer** (Opus mode, worktree isolation)
- **Domain**: All load balancing algorithms, session persistence, backend/node management, connection pooling, outlier detection
- **Crates owned**: `eg-lb`, `eg-pool`, `eg-health`
- **Key skills**: Round robin, weighted round robin (NGINX-style smooth), least connections, least load, random, consistent hash (Murmur3, 150 vnodes), HTTP least response time (EWMA), session persistence (source IP hash, 4-tuple hash, consistent hash, sticky session cookies), circuit breakers, connection draining
- **Must verify**: Lock-free hot paths, correct EWMA cold-start behavior, proper hash ring rebalancing, O(1) session lookups, bounded memory for session maps

#### Agent 8: **Security Engineer** (Opus mode, worktree isolation)
- **Domain**: Network ACLs, rate limiting, TLS fingerprinting (JA3), DDoS protection
- **Crates owned**: `eg-security`
- **Key skills**: Radix trie for IP matching, token bucket rate limiting (connection + packet level), JA3 fingerprint extraction, GREASE filtering, allowlist/denylist modes, LRU-bounded caches
- **Must verify**: O(prefix_length) IP matching, bounded memory under DDoS, correct CIDR handling for IPv4/IPv6, lock-free read path

#### Agent 9: **Control Plane & Config Engineer** (Opus mode, worktree isolation)
- **Domain**: Configuration system, control plane API, node registry, config distribution
- **Crates owned**: `eg-config`, `eg-controlplane`, `eg-agent`
- **Key skills**: TOML parsing (toml crate), hot-reload via inotify/kqueue, axum REST API, tonic gRPC, config versioning, delta sync, heartbeat tracking, graceful drain
- **Must verify**: Atomic config swaps (no partial reads), proper validation before apply, file-based persistence, leader election (later phase)

#### Agent 10: **Observability & Testing Engineer** (Opus mode, worktree isolation)
- **Domain**: Metrics, access logging, compression, proxy protocol, integration tests
- **Crates owned**: `eg-metrics`, `eg-compression`, `eg-proxy-protocol`, `tests/`
- **Key skills**: prometheus crate, structured JSON access logs, brotli/gzip/zstd/deflate, PROXY protocol v1/v2 encode/decode, integration test harness, property-based testing
- **Must verify**: Prometheus exposition format correctness, proper Accept-Encoding negotiation, PROXY protocol one-shot semantics, test coverage > 80%

#### Agent 11: **Reviewer & Auditor** (Opus mode)
- **Role**: Code review, security audit, performance review, RFC compliance verification
- **Responsibilities**: Reviews every PR/change from other agents before merge. Checks for: security vulnerabilities, performance regressions, RFC violations, memory safety issues, API consistency, error handling completeness
- **Does NOT write code** -- only reviews and requests changes

#### Agent 12: **Integration & Bootstrap Engineer** (Opus mode, worktree isolation)
- **Domain**: Binary entry point, lifecycle management, signal handling, graceful shutdown
- **Crates owned**: `eg-bootstrap`
- **Key skills**: tokio runtime setup, XDP program loading, listener binding, graceful drain orchestration, signal handling (SIGTERM, SIGHUP for reload), systemd integration
- **Must verify**: Deterministic startup order, clean shutdown (no connection drops), hot config reload without restart

### Agent Coordination Rules

1. **Task Assignment**: The Architect creates tasks and assigns them to specialists. Each task MUST have clear acceptance criteria.
2. **Worktree Isolation**: Each implementing agent works in an isolated git worktree to prevent merge conflicts. The Architect merges completed work.
3. **Interface-First Development**: Before implementation, agents MUST agree on trait definitions and public API signatures. The Architect reviews all trait definitions.
4. **Review Gate**: No code merges without review from Agent 11 (Reviewer). The Reviewer checks against the feature parity checklist in Section 28.
5. **Cross-Agent Communication**: Agents communicate via the task system. Blocking dependencies MUST be flagged immediately.
6. **Testing Requirement**: Every agent MUST write tests for their code. Unit tests for all public functions, integration tests for cross-crate interactions.

### Spawning Instructions

When starting this project, the Architect agent should:

```
1. Read this entire PROMPT.md
2. Create the Cargo workspace skeleton (Phase 1)
3. Define all shared traits in eg-core (Phase 1)
4. Spawn specialist agents for Phase 2+ work
5. Use `model: "opus"` for all agents
6. Use `isolation: "worktree"` for all implementing agents
7. Assign 5-6 tasks per agent to keep everyone productive
8. Run the Reviewer agent after each phase completion
```

---

## 4. Phase Execution Plan

### Phase 1: Foundation (Architect + Runtime Engineer)
**Goal**: Cargo workspace, core traits, runtime abstraction, config system

1. Create workspace with all crate stubs
2. Define core traits: `LoadBalancer`, `SessionPersistence`, `HealthChecker`, `Backend`, `Node`, `Connection`, `Listener`, `Protocol`
3. Implement `eg-runtime` with io_uring backend and epoll fallback
4. Implement `eg-config` with TOML parsing and hot-reload
5. Implement `eg-core` error types, common utilities

**Acceptance**: `cargo build` succeeds for all crates, runtime can accept TCP connections

### Phase 2: L4 Data Plane (XDP Engineer + LB Engineer)
**Goal**: XDP-accelerated TCP/UDP forwarding

1. Implement XDP programs for TCP and UDP forwarding
2. Implement BPF map management (connection table, session table, backend table)
3. Implement L4 load balancing algorithms (all 6)
4. Implement session persistence (source IP hash, 4-tuple hash, consistent hash)
5. Wire XDP userspace loader to runtime

**Acceptance**: TCP and UDP traffic forwarded through XDP at line rate with load balancing

### Phase 3: TLS & Security (TLS Engineer + Security Engineer)
**Goal**: Full TLS stack, ACLs, rate limiting

1. Implement TLS termination with rustls (TLS 1.2, 1.3)
2. Implement SNI-based certificate selection
3. Implement OCSP stapling with 6-hour refresh
4. Implement mTLS (REQUIRED, OPTIONAL, NOT_REQUIRED)
5. Implement certificate hot-relod (atomic swap)
6. Implement NACL (radix trie, allowlist/denylist)
7. Implement connection rate limiting (token bucket)
8. Implement packet rate limiting (UDP)
9. Implement TLS fingerprinting (JA3)

**Acceptance**: TLS handshake with SNI, mTLS validation, rate limiting under load, JA3 extraction

### Phase 4: HTTP/1.1 Proxy (HTTP Engineer + Compression Engineer)
**Goal**: Full HTTP/1.1 reverse proxy

1. Implement HTTP/1.1 request/response proxying (hyper)
2. Implement keep-alive and connection pooling (H1)
3. Implement chunked encoding
4. Implement hop-by-hop header stripping (RFC 9110 Section 7.6.1)
5. Implement header manipulation (Via, X-Request-ID, X-Forwarded-For/Proto)
6. Implement 100-continue handling
7. Implement request body size limits and timeouts (Slowloris/slow-POST defense)
8. Implement content compression (brotli, gzip, zstd, deflate)
9. Implement access logging (JSON structured)
10. Implement URI normalization (RFC 3986, path traversal prevention)

**Acceptance**: Full HTTP/1.1 proxying with compression, timeouts, and access logs

### Phase 5: HTTP/2 Proxy (HTTP Engineer + LB Engineer)
**Goal**: Full HTTP/2 reverse proxy with per-stream load balancing

1. Implement HTTP/2 stream multiplexing (h2 crate)
2. Implement HPACK header compression
3. Implement flow control (connection + stream level)
4. Implement h2c (HTTP/2 cleartext, prior knowledge)
5. Implement CONNECT tunneling (RFC 9113 Section 8.5)
6. Implement control frame flood protection (SETTINGS + PING rate limit)
7. Implement per-stream and per-connection body size limits
8. Implement GOAWAY for graceful shutdown
9. Implement H1<->H2 protocol translation (both directions)
10. Implement per-stream load balancing

**Acceptance**: HTTP/2 multiplexed proxying with flow control, protocol translation

### Phase 6: HTTP/3 & QUIC (QUIC Engineer)
**Goal**: Full HTTP/3 reverse proxy over QUIC

1. Implement QUIC transport (quinn)
2. Implement HTTP/3 request/response handling (h3)
3. Implement QUIC connection pooling
4. Implement stateless retry with token validation
5. Implement connection migration and NAT rebinding detection
6. Implement 0-RTT support with replay protection
7. Implement Alt-Svc header injection (RFC 7838)
8. Implement H1<->H3, H2<->H3 protocol translation
9. Implement QUIC flow control (connection + stream level)
10. Implement connection ID routing

**Acceptance**: HTTP/3 proxying over QUIC with 0-RTT, migration, and Alt-Svc

### Phase 7: gRPC & WebSocket (gRPC/WS Engineer)
**Goal**: Full gRPC and WebSocket reverse proxy

1. Implement gRPC detection (Content-Type: application/grpc)
2. Implement gRPC status code mapping (HTTP -> gRPC, 14 mappings)
3. Implement gRPC deadline parsing and enforcement (grpc-timeout header, max 300s)
4. Implement all streaming types (unary, server, client, bidirectional)
5. Implement gRPC trailer forwarding (grpc-status, grpc-message)
6. Implement gRPC health check endpoint (/grpc.health.v1.Health/Check)
7. Implement gRPC-aware compression
8. Implement WebSocket upgrade (HTTP/1.1)
9. Implement WebSocket frame handling (Text, Binary, Ping, Pong, Close, Continuation)
10. Implement WebSocket-over-HTTP/2 (RFC 8441, Extended CONNECT)
11. Implement WebSocket backpressure (bidirectional flow control)
12. Implement WebSocket idle timeout with Close frame (code 1001)

**Acceptance**: gRPC proxying with all streaming types, WebSocket with backpressure

### Phase 8: L7 Load Balancing & Routing (LB Engineer)
**Goal**: HTTP-aware load balancing with routing rules

1. Implement HTTP round robin
2. Implement HTTP consistent hash (configurable hash key: header or client IP)
3. Implement HTTP random
4. Implement HTTP least response time (EWMA with cold-start, 10-sample threshold, alpha 0.5)
5. Implement sticky session (cookie: X-SBZ-EGW-RouteID, SHA-256 hashed node ID)
6. Implement host-based routing (literal + wildcard matching)
7. Implement path-based routing (glob patterns)
8. Implement header-based routing (key-value matching)
9. Implement priority-based rule evaluation
10. Implement health endpoint (/health liveness, /ready readiness)

**Acceptance**: All L7 LB algorithms with routing rules, sticky sessions, and health endpoints

### Phase 9: Control Plane (CP Engineer)
**Goal**: Config management with REST + gRPC APIs

1. Implement file-based config store (TOML)
2. Implement config versioning and rollback
3. Implement REST API (axum): clusters, listeners, routes, health-checks, TLS certs, nodes, status, config
4. Implement gRPC services (tonic): NodeRegistration, ConfigDistribution, NodeControl
5. Implement session token authentication for gRPC
6. Implement heartbeat tracking (miss threshold, disconnect threshold)
7. Implement graceful drain with RECONNECT directives
8. Implement node registry with state machine (CONNECTED -> UNHEALTHY -> DISCONNECTED)
9. Implement config distribution (delta sync with journal)
10. Implement reconnect storm protection (token bucket)

**Acceptance**: Full REST + gRPC control plane, config hot-reload, node lifecycle

### Phase 10: Integration, Testing & Hardening (All Agents)
**Goal**: End-to-end testing, performance validation, security hardening

1. Integration tests for all protocol combinations
2. Security tests (request smuggling, CRLF injection, control frame floods, slowloris)
3. Performance benchmarks (throughput, latency percentiles)
4. Chaos tests (backend failures, config corruption, network partitions)
5. RFC compliance validation
6. Memory safety audit (no leaks under sustained load)
7. Full feature parity verification against Section 28 checklist

**Acceptance**: All tests pass, performance meets requirements (Section 26), zero security findings

---

## 5. Module 1: Foundation & Build System

### Cargo Workspace Configuration

```toml
[workspace]
resolver = "2"
members = [
    "crates/eg-core",
    "crates/eg-config",
    "crates/eg-xdp",
    "crates/eg-runtime",
    "crates/eg-tls",
    "crates/eg-lb",
    "crates/eg-health",
    "crates/eg-pool",
    "crates/eg-security",
    "crates/eg-protocol-tcp",
    "crates/eg-protocol-udp",
    "crates/eg-protocol-http",
    "crates/eg-protocol-h3",
    "crates/eg-protocol-grpc",
    "crates/eg-protocol-ws",
    "crates/eg-compression",
    "crates/eg-proxy-protocol",
    "crates/eg-metrics",
    "crates/eg-controlplane",
    "crates/eg-agent",
    "crates/eg-bootstrap",
]

[workspace.dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
io-uring = "0.7"
# XDP/eBPF
aya = "0.13"
aya-bpf = "0.1"
# TLS
rustls = { version = "0.23", features = ["ring"] }
tokio-rustls = "0.26"
webpki-roots = "0.26"
# HTTP
hyper = { version = "1", features = ["full"] }
hyper-util = { version = "0.1", features = ["full"] }
h2 = "0.4"
# QUIC / HTTP3
quinn = "0.11"
h3 = "0.0.6"
h3-quinn = "0.0.7"
# gRPC
tonic = "0.12"
prost = "0.13"
# WebSocket
tokio-tungstenite = "0.24"
# Config
toml = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# Compression
brotli = "7"
flate2 = "1"
zstd = "0.13"
# Metrics
prometheus = "0.13"
# Crypto
ring = "0.17"
# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
# Concurrency
crossbeam = "0.8"
dashmap = "6"
parking_lot = "0.12"
# Utilities
bytes = "1"
thiserror = "2"
anyhow = "1"
uuid = { version = "1", features = ["v4"] }
```

### Minimum Supported Rust Version (MSRV)

Use the **latest stable Rust** (1.87+ as of writing). No MSRV pinning -- we want access to the newest language features.

### Core Traits (`eg-core`)

Define these foundational traits that all modules implement:

```rust
// These are the key abstractions. Actual implementations are in respective crates.

/// A backend node that can receive traffic
pub trait Node: Send + Sync {
    fn id(&self) -> &str;
    fn address(&self) -> SocketAddr;
    fn state(&self) -> NodeState;
    fn set_state(&self, state: NodeState);
    fn active_connections(&self) -> u64;
    fn max_connections(&self) -> Option<u64>;  // None = unlimited
    fn bytes_sent(&self) -> u64;
    fn bytes_received(&self) -> u64;
    fn health(&self) -> Health;
}

/// Load balancer that selects a backend node
pub trait LoadBalancer<Req, Resp>: Send + Sync {
    fn select(&self, request: &Req) -> Result<Resp>;
    fn add_node(&self, node: Arc<dyn Node>);
    fn remove_node(&self, node_id: &str);
    fn online_nodes(&self) -> Vec<Arc<dyn Node>>;
}

/// Session persistence for sticky routing
pub trait SessionPersistence<K, V>: Send + Sync {
    fn get(&self, key: &K) -> Option<V>;
    fn put(&self, key: K, value: V);
    fn remove(&self, key: &K);
}

/// Health checker for backend nodes
pub trait HealthChecker: Send + Sync {
    async fn check(&self, node: &dyn Node) -> HealthCheckResult;
}

/// Connection to a backend
pub trait Connection: Send + Sync {
    fn state(&self) -> ConnectionState;
    fn is_usable(&self) -> bool;
    async fn close(&self) -> Result<()>;
}

/// Listener that accepts incoming connections
pub trait Listener: Send + Sync {
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    fn is_draining(&self) -> bool;
}
```

---

## 6. Module 2: XDP Data Plane (L4)

### Overview

The XDP layer handles L4 (TCP/UDP) packet forwarding entirely in-kernel using eBPF programs attached to the NIC driver. This eliminates kernel-to-userspace copies for pure L4 forwarding.

### XDP Program Architecture

```
Packet arrives at NIC
        |
        v
+------------------+
| XDP Program      |
|                  |
| 1. Parse headers |
| 2. ACL check     |  --> XDP_DROP (blocked by ACL)
| 3. Rate limit    |  --> XDP_DROP (rate exceeded)
| 4. Lookup conn   |
|    table         |
| 5a. Known conn:  |  --> Rewrite headers, XDP_TX/XDP_REDIRECT
| 5b. New conn:    |  --> XDP_PASS (to userspace for LB decision)
| 5c. L7 traffic:  |  --> XDP_PASS (to io_uring handlers)
+------------------+
```

### BPF Maps Required

| Map Name | Type | Key | Value | Max Entries | Purpose |
|----------|------|-----|-------|-------------|---------|
| `tcp_conn_table` | BPF_MAP_TYPE_HASH | `(src_ip, src_port, dst_ip, dst_port)` | `(backend_ip, backend_port, state)` | 1,000,000 | TCP connection tracking |
| `udp_session_table` | BPF_MAP_TYPE_HASH | `(src_ip, src_port)` | `(backend_ip, backend_port, last_seen)` | 500,000 | UDP session affinity |
| `backend_table` | BPF_MAP_TYPE_ARRAY | `u32` (index) | `(ip, port, weight, state)` | 10,000 | Backend server list |
| `acl_trie` | BPF_MAP_TYPE_LPM_TRIE | `(prefix_len, ip)` | `(action, rate_limit_id)` | 100,000 | IP ACL with CIDR support |
| `rate_limit` | BPF_MAP_TYPE_HASH | `src_ip` | `(tokens, last_refill, config_id)` | 100,000 | Per-IP rate limiting |
| `config` | BPF_MAP_TYPE_ARRAY | `u32` (key) | `ConfigValue` | 64 | Runtime configuration |
| `stats` | BPF_MAP_TYPE_PERCPU_ARRAY | `u32` (stat_id) | `u64` (counter) | 32 | Per-CPU statistics |
| `l7_ports` | BPF_MAP_TYPE_HASH | `u16` (port) | `u8` (flags) | 256 | Ports that need L7 processing (XDP_PASS) |

### XDP Program Implementation Requirements

1. **Packet Parsing**: Ethernet -> IPv4/IPv6 -> TCP/UDP. Handle VLAN tags.
2. **Header Rewrite**: Source/destination IP and port rewrite. Recalculate IP checksum (incremental), TCP/UDP checksum (incremental using RFC 1624).
3. **Connection Tracking**: On new TCP SYN, insert into `tcp_conn_table` after userspace LB decision populates the entry. On FIN/RST, mark for cleanup.
4. **Session Expiry**: Userspace periodically scans `udp_session_table` and removes entries older than 30 seconds.
5. **L7 Bypass**: If destination port is in `l7_ports` map, return `XDP_PASS` immediately. This is how HTTP/HTTPS/QUIC traffic reaches io_uring handlers.
6. **Verifier Safety**: All memory accesses must have bounds checks. No unbounded loops. Use `bpf_loop()` helper for iteration (kernel 5.17+).
7. **Atomic Operations**: Use `__sync_fetch_and_add` for statistics counters.

### Userspace XDP Manager

The userspace component manages the XDP program lifecycle:

```rust
pub struct XdpManager {
    // Loaded XDP programs
    programs: HashMap<String, XdpProgram>,
    // BPF map handles for userspace read/write
    maps: BpfMaps,
    // Interface attachment
    interface: String,
    // XDP flags (SKB_MODE for testing, DRV_MODE fr production)
    mode: XdpMode,
}

impl XdpManager {
    /// Load and attach XDP programs
    pub fn attach(&mut self, interface: &str) -> Result<()>;
    /// Detach XDP programs (graceful shutdown)
    pub fn detach(&mut self) -> Result<()>;
    /// Update backend table from load balancer
    pub fn update_backends(&self, backends: &[BackendEntry]) -> Result<()>;
    /// Insert new connection mapping (called after L4 LB decision)
    pub fn insert_tcp_connection(&self, key: &FourTuple, backend: &BackendAddr) -> Result<()>;
    /// Update ACL rules
    pub fn update_acl(&self, rules: &[AclRule]) -> Result<()>;
    /// Read per-CPU statistics
    pub fn read_stats(&self) -> Result<XdpStats>;
}
```

### Fallback Mode

When XDP is unavailable (non-Linux, insufficient kernel version, missing driver support):
- Fall back to userspace L4 proxying via io_uring/epoll
- Log warning: "XDP unavailable, falling back to userspace L4 proxy. Performance will be reduced."
- The `eg-protocol-tcp` and `eg-protocol-udp` crates handle userspace L4 proxying

---

## 7. Module 3: io_uring Async Runtime

### Runtime Abstraction

Provide a unified async I/O interface that uses io_uring when available and falls back to epoll:

```rust
pub enum RuntimeBackend {
    IoUring,
    Epoll,
}

pub struct Runtime {
    backend: RuntimeBackend,
    // ... internal state
}

impl Runtime {
    /// Auto-detect best available backend
    pub fn new() -> Result<Self>;
    /// Force a specific backend
    pub fn with_backend(backend: RuntimeBackend) -> Result<Self>;
    /// Run the async event loop
    pub async fn run<F: Future>(&self, future: F) -> F::Output;
}
```

### io_uring Features to Use

1. **IORING_OP_ACCEPT** with multishot for connection acceptance (reduces syscalls)
2. **IORING_OP_RECV / IORING_OP_SEND** for data transfer
3. **IORING_OP_SPLICE** for zero-copy proxying between sockets
4. **IORING_OP_TIMEOUT** for connection timeouts
5. **IORING_OP_CANCEL** for timeout cancellation
6. **Fixed file descriptors** (`IORING_REGISTER_FILES`) to avoid fd table lookups
7. **Buffer pools** (`IORING_REGISTER_BUFFERS`) for pre-registered I/O buffers
8. **SQ polling** (`IORING_SETUP_SQPOLL`) for kernel-side submission (optional, CPU-intensive)

### Socket Options (Match Java Implementation)

Apply these socket options on all listener and backend connections:

**Listener (Server) Sockets:**
- `SO_REUSEADDR`: true
- `SO_REUSEPORT`: true (multi-socket binding for multi-core scaling)
- `SO_RCVBUF`: 262,144 (256 KB)
- `SO_SNDBUF`: 262,144 (256 KB)
- `TCP_NODELAY`: true (disable Nagle's algorithm)
- `TCP_QUICKACK`: true (disable delayed ACK, Linux only)
- `SO_KEEPALIVE`: true
- `TCP_FASTOPEN`: queue length from config (TCP Fast Open)
- `SO_BACKLOG`: 50,000

**Backend (Client) Sockets:**
- `TCP_NODELAY`: true
- `SO_KEEPALIVE`: true
- `SO_RCVBUF`: 262,144
- `SO_SNDBUF`: 262,144
- `TCP_QUICKACK`: true
- `TCP_FASTOPEN_CONNECT`: true (client-side TFO)

**UDP Sockets:**
- `SO_REUSEPORT`: true
- `UDP_GRO`: true (Generic Receive Offload, Linux 5.0+)

### Backpressure Model

Match the Java implementation's write buffer water marks:
- **High water mark**: 65,536 bytes (64 KB) -- pause reads when write buffer exceeds this
- **Low water mark**: 32,768 bytes (32 KB) -- resume reads when write buffer drains below this
- Implement bidirectional backpressure: when backend is slow, pause client reads; when client is slow, pause backend reads

---

## 8. Module 4: TLS & Cryptography

### TLS Implementation Requirements

Use **rustls** as the TLS implementation. Do NOT use OpenSSL.

#### TLS Versions
- **TLS 1.3**: Supported, preferred
- **TLS 1.2**: Supported
- **TLS 1.1**: Deprecated. MUST reject in production mode with error message.
- **TLS 1.0**: Not supported.

#### Cipher Suites

**Modern Profile (TLS 1.3 only):**
- `TLS_AES_256_GCM_SHA384`
- `TLS_AES_128_GCM_SHA256`
- `TLS_CHACHA20_POLY1305_SHA256`

**Intermediate Profile (TLS 1.2 + 1.3):**
- All modern ciphers plus:
- `TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384`
- `TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384`
- `TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256`
- `TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256`

**Production Security Enforcement:**
- Reject anonymous DH/ECDH ciphers (no authentication)
- Reject non-PFS RSA ciphers (no forward secrecy)
- Warn on CBC mode ciphers (POODLE variants)
- Reject DSA keys

#### SNI (Server Name Indication)
- Extract hostname from TLS ClientHello
- Look up certificate by exact hostname or wildcard pattern (`*.example.com`)
- Default certificate when no match found
- Store SNI hostname for downstream use (routing, sticky sessions)
- Handshake timeout: 10 seconds (Slowloris defense)

#### ALPN (Application-Layer Protocol Negotiation)
- Negotiate protocol during TLS handshake
- Supported protocols: `h2`, `http/1.1`
- Store negotiated protocol for pipeline selection
- Fatal alert on ALPN failure (RFC 7301)

#### OCSP Stapling
- Fetch OCSP response from responder URL (extracted from AIA extension)
- 16-byte SecureRandom nonce per RFC 8954
- Verify response nonce matches request (replay prevention)
- Verify response signature using issuer certificate
- Refresh every 6 hours
- Thread-safe: volatile/atomic OCSP data, clone on read
- Minimum 2 certificates required (end-entity + issuer)

#### Mutual TLS (mTLS)
Three modes matching Java implementation:
- `NOT_REQUIRED` (default): No client certificate requested
- `OPTIONAL`: Client certificate requested but connection allowed without
- `REQUIRED`: Client certificate mandatory, fatal alert if missing

Support custom trust store (PEM file) and optional CRL (Certificate Revocation List).

#### Certificate Hot-Reload
- Use `RwLock` for atomic `ServerConfig` swaps
- Write lock only during reload, read lock for TLS handshakes
- Existing connections continue with negotiated TLS sessions
- Only new connections use updated certificates
- 30-day expiration warning with detailed logging

#### Certificate Formats
- PEM format for certificates and keys
- PKCS#1 and PKCS#8 private key formats
- RSA and EC key types (reject DSA)
- X.509 certificate parsing

---

## 9. Module 5: L4 Load Balancing & Session Persistence

### Load Balancing Algorithms (L4)

Implement ALL six algorithms from the Java codebase:

#### 1. Round Robin
- Atomic index counter with modulo wrap
- Lock-free via `AtomicUsize::fetch_add`
- Decrement max index on node removal

#### 2. Weighted Round Robin (NGINX-style smooth)
- Per-node state: `current_weight`, `effective_weight`
- Selection: For each node, `current_weight += effective_weight`, select highest, `current_weight -= total_weight`
- Produces smooth interleaved distribution (e.g., weights {5,1,1} -> A,A,B,A,C,A,A)
- Runtime weight updates via `set_weight(node, weight)`
- Synchronized (Mutex) for atomic read-modify-write of all node weights

#### 3. Least Connection
- Select node with fewest `active_connections()`
- O(n) scan across online nodes
- Atomic counter for connection tracking

#### 4. Least Load
- Combine connection count and load percentage: `active_connections / max_connections`
- Lowest load first, connection count as tiebreaker

#### 5. Random
- `ThreadRng`-based selection from online nodes
- Thread-safe (thread-local RNG)

#### 6. Consistent Hash
- TreeMap (BTreeMap) hash ring with 150 virtual nodes per real node
- Murmur3-128 hash function (use `murmur3` crate)
- `RwLock` protection: concurrent reads, exclusive writes
- Client socket address (IP + port) as hash key
- Graceful fallback: walk ring to find next online node

### Session Persistence (L4)

#### 1. Source IP Hash
- /24 subnet mask for IPv4 (groups 256 clients)
- /48 subnet mask for IPv6
- TTL: 1 hour, max entries: 100,000
- Evict ~10% in batch when at capacity
- Use `dashmap` with TTL wrapper or custom expiring map

#### 2. Four-Tuple Hash
- Key: (src_ip, src_port, dst_ip, dst_port)
- TTL: 1 hour, max entries: 500,000
- Batch eviction under capacity pressure

#### 3. Consistent Hash Persistence
- Stateless -- no per-session storage, only ring state
- Hash client source IP (without port) to ring
- ~1/N sessions remapped on node changes
- `RwLock` for ring mutations

### Node Management

```rust
pub struct NodeImpl {
    id: String,
    address: SocketAddr,
    state: AtomicCell<NodeState>,  // ONLINE, OFFLINE, IDLE, MANUAL_OFFLINE
    active_connections: AtomicU64,
    max_connections: Option<u64>,  // None = unlimited (default 10,000)
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    circuit_breaker: CircuitBreaker,
    health_check: Option<Arc<dyn HealthChecker>>,
}
```

- Thread-safe state transitions
- TOCTOU protection: atomic check + insert for connection limits
- Connection draining with HTTP/2 GOAWAY support (5-second grace period)
- O(1) connection count via atomic counter

### Connection Backlog Queue

When backend connection is still establishing:
- Queue client data in bounded `ConcurrentQueue` (max 10,000 items)
- O(1) size tracking via `AtomicUsize`
- Memory pressure adaptive limit: effective limit = max/4 under pressure
- Drain in batches of 64 with writability checks
- Clear backlog on connection failure (release all BytesMut)

---

## 10. Module 6: HTTP/1.1 Proxy

### Request/Response Handling (RFC 9110 / RFC 9112)

Use **hyper 1.x** for HTTP/1.1 parsing and serialization.

#### Features to Implement

1. **Keep-Alive**: Persistent connections with configurable idle timeout. Connection pooling per backend.
2. **Chunked Encoding**: Bidirectional chunked transfer encoding.
3. **Pipelining**: Serial response ordering with `request_in_flight` flag and pending queue.
4. **100-Continue**: Handle `Expect: 100-continue` header per RFC 9110 Section 8.6.
5. **Connection Close Detection**: Track `Connection: close` from backend.
6. **Graceful Draining**: `start_draining()` flag preventing new requests, complete in-flight.

#### Header Manipulation

**Hop-by-Hop Header Stripping** (RFC 9110 Section 7.6.1 & RFC 9113 Section 8.2.2):
- Static set: `connection`, `keep-alive`, `te`, `trailers`, `transfer-encoding`, `upgrade`, `proxy-connection`, `proxy-authenticate`, `proxy-authorization`
- Dynamic: parse `Connection` header value for additional hop-by-hop names
- Preserve `Upgrade` header only for valid WebSocket upgrades
- Zero-allocation comma scanning for Connection header parsing

**Custom Header Injection:**
- `Via`: Protocol version and proxy identifier
- `X-Request-ID`: Unique per-request identifier (UUID v4 or TSID)
- `X-Forwarded-For`: Client IP address
- `X-Forwarded-Proto`: Original protocol (http/https)

#### Request Body Handling

- Per-request max body size limit, return 413 on exceed
- Request header timeout (Slowloris defense): absolute deadline for complete headers
- Request body timeout (slow-POST defense): absolute deadline for complete body
- Stream body chunk-by-chunk without full buffering

#### Error Responses

- 400 Bad Request: Malformed HTTP
- 413 Content Too Large: Oversized body (RFC 9110 Section 15.5.14)
- 414 URI Too Long: Oversized request line
- 431 Request Header Fields Too Large: Oversized headers
- 502 Bad Gateway: Backend connection failure
- 503 Service Unavailable: No healthy backend or draining
- 504 Gateway Timeout: Backend request timeout

#### Health Endpoints

- `GET /health`: Liveness -- 200 if not draining, 503 if draining
- `GET /ready`: Readiness -- 200 if at least one backend online, 503 otherwise

#### URI Normalization (RFC 3986 Section 5.2.4)

- Remove dot segments (`/.` and `/..`)
- Path traversal detection: reject `/../` escape
- Null byte rejection: reject `%00` in path
- Double-encoded dot detection: reject `%252e`
- Preserve query string

---

## 11. Module 7: HTTP/2 Proxy

### Stream Multiplexing (RFC 9113)

Use **h2** crate for HTTP/2 frame handling.

#### Features to Implement

1. **Stream State Machine**: OPEN, HALF_CLOSED_REMOTE, HALF_CLOSED_LOCAL, CLOSED
2. **Concurrent Stream Limit**: Configurable `SETTINGS_MAX_CONCURRENT_STREAMS`
3. **Stream ID Management**: Client=odd, server=even per RFC 9113 Section 5.1.1
4. **Per-Stream Load Balancing**: Each H2 stream independently routed to different backends

#### HPACK Header Compression
- Automatic via h2 crate
- Configurable header table sizes via SETTINGS frames

#### Flow Control (RFC 9113 Section 6.9)
- Connection-level window: configurable via `h2_connection_window_size`
- Stream-level window: default 65,535 bytes, configurable via SETTINGS
- Automatic WINDOW_UPDATE management
- Backpressure propagation: when backend unwritable, pause frontend reads
- Multi-backend aggregation: for multiplexed H2 fanning to multiple backends, resume only when ALL backends writable

#### Control Frame Flood Protection
- Rate-limit SETTINGS + PING: max 100 per 10-second window
- GOAWAY with ENHANCE_YOUR_CALM (0xb) on violation per RFC 9113 Section 10.5

#### Body Size Limits
- Per-stream limit: `max_request_body_size`
- Per-connection aggregate: `max_connection_body_size` (default 256 MB)
- Per-stream response body limit: `max_response_body_size`

#### h2c (HTTP/2 Cleartext, RFC 9113 Section 3.4)
- Detect 24-byte connection preface: `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n`
- Switch to HTTP/2 codec on preface match
- Fall back to HTTP/1.1 for non-matching data

#### CONNECT Tunneling (RFC 9113 Section 8.5)
- Only `:method` and `:authority` pseudo-headers in CONNECT
- No `:scheme` or `:path`
- Bidirectional byte forwarding after establishment

#### Protocol Translation

**H1 -> H2:**
- Request line + headers -> pseudo-headers (`:method`, `:path`, `:scheme`, `:authority`) + HEADERS frame
- Chunked body -> DATA frames
- Host header -> `:authority`
- Scheme from TLS context or X-Forwarded-Proto

**H2 -> H1:**
- Pseudo-headers -> request line
- DATA frames -> chunked body
- Trailer HEADERS -> chunked trailers

#### Graceful Shutdown
- Send GOAWAY with NO_ERROR
- Wait for in-flight streams to complete (configurable drain timeout)
- Then close connection

---

## 12. Module 8: HTTP/3 & QUIC Proxy

### QUIC Transport (RFC 9000)

Use **quinn** for QUIC transport and **h3** + **h3-quinn** for HTTP/3.

#### QUIC Features

1. **Connection Pooling**: Per-backend QUIC connection pool, reuse connections across streams
2. **Stateless Retry**: Token validation with AES-GCM encryption, expiration check
3. **Connection Migration**: Handle client IP changes, NAT rebinding detection
4. **Path Validation**: Verify new paths before migrating
5. **0-RTT Support**: Early data with replay protection
6. **Connection ID Routing**: Route packets to correct handler by DCID
7. **Flow Control**: Connection-level (MAX_DATA) and stream-level (MAX_STREAM_DATA)

### HTTP/3 Features (RFC 9114)

1. **Request/Response Semantics**: QPACK header compression
2. **Stream Multiplexing**: Over QUIC transport
3. **Alt-Svc Header Injection**: Advertise HTTP/3 availability via RFC 7838
   - Format: `Alt-Svc: h3=":PORT"; ma=MAX_AGE`
   - Inject into HTTP/1.1 and HTTP/2 responses
4. **GOAWAY**: H3_NO_ERROR (0x100) for graceful shutdown
5. **Per-Backend Active Stream Metrics**

### Protocol Translation

Implement all translation paths:
- H1 <-> H3
- H2 <-> H3
- Same pseudo-header structure as H2 for H3

### TLS Requirement

QUIC mandates TLS 1.3. Enforce this at connection setup.

---

## 13. Module 9: gRPC Proxy

### gRPC Detection

- Content-Type starts with `application/grpc`
- Transparent proxying over HTTP/2 (and HTTP/3)

### gRPC Status Code Mapping (HTTP -> gRPC)

| HTTP Status | gRPC Code | Name |
|------------|-----------|------|
| 200 | 0 | OK |
| 400 | 3 | INVALID_ARGUMENT |
| 401 | 16 | UNAUTHENTICATED |
| 403 | 7 | PERMISSION_DENIED |
| 404 | 12 | UNIMPLEMENTED |
| 408 | 4 | DEADLINE_EXCEEDED |
| 409 | 10 | ABORTED |
| 429 | 8 | RESOURCE_EXHAUSTED |
| 499 | 1 | CANCELLED |
| 500 | 13 | INTERNAL |
| 501 | 2 | UNIMPLEMENTED |
| 502 | 14 | UNAVAILABLE |
| 503 | 14 | UNAVAILABLE |
| 504 | 4 | DEADLINE_EXCEEDED |

### gRPC Deadline Handling

- Parse `grpc-timeout` header: format `[0-9]+[HMSmun]` (Hours, Minutes, Seconds, millis, micros, nanos)
- Clamp maximum deadline to 300 seconds
- Schedule deadline enforcement with timeout task
- Cancel deadline on stream completion (RST_STREAM, DATA endStream, response trailers)
- Per-stream deadline tracking via `HashMap<StreamId, JoinHandle>`

### Streaming Types

All four gRPC streaming types must work transparently:
1. **Unary**: Single request, single response
2. **Server Streaming**: Single request, multiple responses
3. **Client Streaming**: Multiple requests, single response
4. **Bidirectional Streaming**: Multiple requests and responses simultaneously

### gRPC Trailers

- Detect trailing HEADERS frame after DATA frames
- Preserve `grpc-status`, `grpc-message`, `grpc-status-details-bin`
- Validate trailers include `grpc-status`

### gRPC Health Check Endpoint

- Path: `/grpc.health.v1.Health/Check`
- Respond directly with protobuf health response
- Length-prefixed message (5-byte header: compression flag + 4-byte length)

### gRPC-Aware Compression

- Respect `grpc-encoding` header
- Support: gzip, deflate, identity
- Do NOT compress if grpc-encoding specifies identity

---

## 14. Module 10: WebSocket Proxy

### WebSocket Upgrade (RFC 6455)

- Detect HTTP/1.1 upgrade: `Connection: Upgrade` + `Upgrade: websocket`
- Full upgrade handshake flow
- Use `tokio-tungstenite` for WebSocket frame handling

### Frame Handling

- **Text frames**: UTF-8 text data
- **Binary frames**: Raw binary data
- **Ping frames**: Respond with Pong locally (prevent duplicate Pongs from backend)
- **Pong frames**: Forward to backend
- **Close frames**: Graceful close handshake with status code
- **Continuation frames**: Multi-frame message assembly
- **Max frame payload**: 64 KB default, configurable
- **Oversize rejection**: Status code 1009 (Message Too Big)

### WebSocket-over-HTTP/2 (RFC 8441)

- Extended CONNECT method
- Stream multiplexing for multiple WebSocket connections over single H2 connection
- CONNECT-specific handling in H2 HEADERS

### Backpressure

- Bidirectional flow control
- Pause client reads when backend unwritable
- Resume reads when backend drains
- Check writability after each frame write

### Idle Timeout

- Configurable idle timeout (default 300 seconds)
- Send Close frame with code 1001 (Going Away) on idle timeout

---

## 15. Module 11: L7 Load Balancing & Routing

### HTTP Load Balancing Algorithms

Implement ALL four L7 algorithms from Java:

#### 1. HTTP Round Robin
- Same as L4 but returns `HttpBalanceResponse` with optional headers

#### 2. HTTP Consistent Hash
- Configurable hash key source: HTTP header value OR client IP
- Support HTTP/1.1 headers and HTTP/2 pseudo-headers (`:authority`, etc.)
- Fall back to client IP if header absent
- 150 virtual nodes, Murmur3-128, BTreeMap ring

#### 3. HTTP Random
- ThreadLocal RNG selection from online nodes

#### 4. HTTP Least Response Time (EWMA)
- Exponentially Weighted Moving Average response time tracking
- Cold-start handling: round-robin for first 10 samples, then EWMA
- Three-tier selection:
  - All cold: round-robin
  - Some cold: prefer cold nodes (gather samples)
  - All warm: select lowest EWMA
- Lock-free EWMA updates via CAS loops
- Alpha parameter: 0.5 (configurable)

### Sticky Session (Cookie-Based)

- Cookie name: `X-SBZ-EGW-RouteID`
- Cookie value: SHA-256 hash of node ID (prevents UUID leakage)
- O(1) lookup via pre-computed `hashed_id_to_node` map
- Cookie attributes:
  - Domain: from Host header (strip port per RFC 6265)
  - Path: `/`
  - HttpOnly: true
  - SameSite: Strict
  - Secure: true when TLS enabled
- Support both HTTP/1.1 Host header and HTTP/2 `:authority`

### Routing Rules

```rust
pub struct RouteConfig {
    /// Host matching: literal or wildcard (e.g., "*.example.com")
    pub host: Option<String>,
    /// Path matching: glob pattern (e.g., "/api/*")
    pub path: Option<String>,
    /// Header matching: key-value pairs
    pub headers: Option<HashMap<String, String>>,
    /// Priority: lower value = higher precedence
    pub priority: u32,
    /// Target backend cluster
    pub cluster_id: String,
}
```

- Evaluate rules in priority order
- First match wins
- Fallback to default cluster if no match

---

## 16. Module 12: Health Checking & Circuit Breakers

### Active Health Checks

#### TCP Health Check
- Open socket, attempt connect with configurable timeout
- Success = connected, Failure = timeout/refused

#### UDP Health Check
- Send "PING" datagram
- Expect "PING" or "PONG" response within timeout

#### HTTP Health Check
- Send configurable HTTP request (method, path, headers)
- Validate response status code (configurable expected codes)
- Application-level health check

### Health Check Configuration

```rust
pub struct HealthCheckConfig {
    pub interval: Duration,        // Check interval
    pub timeout: Duration,         // Per-check timeout
    pub rise: u32,                 // Consecutive successes for ONLINE (default 2)
    pub fall: u32,                 // Consecutive failures for OFFLINE (default 3)
    pub samples: u32,              // Sample window size (default 100)
}
```

### Health States

- **GOOD**: >= 95% successful checks
- **MEDIUM**: 75-95% successful checks
- **BAD**: < 75% successful checks
- **UNKNOWN**: No samples yet

### Health Check Caching

- 5-second cache TTL
- Atomic cached health value for lock-free reads
- Double-check pattern for cache refresh

### Exponential Backoff

- Base: 1000ms, Max: 60,000ms
- Multiplier: 2^(failures-1)
- Reset to 0 on success

### Circuit Breaker

State machine:
```
CLOSED --(failure_threshold failures)--> OPEN
OPEN --(open_duration elapsed)--> HALF_OPEN
HALF_OPEN --(success_threshold successes)--> CLOSED
HALF_OPEN --(any failure)--> OPEN
```

- Atomic state via `AtomicU8` / enum
- Configurable thresholds and durations
- Limited concurrent requests in HALF_OPEN state
- CAS-based state transitions (lock-free)

### Outlier Detection (Passive Health Checking)

- Monitor success/failure from actual traffic
- Per-node consecutive failure counter
- Two-phase evaluation:
  1. Check for restoration (ejection time elapsed)
  2. Check for ejection (threshold exceeded)
- Eject to OFFLINE, restore to IDLE (for active health check re-verification)
- Max ejection percentage cap (prevent entire pool removal)
- Thread-safe via `DashMap` and `AtomicU64`

---

## 17. Module 13: Security & DDoS Protection

### Network Access Control List (NACL)

- **Dual-mode**: ALLOWLIST (default deny) or DENYLIST (default allow)
- **Data structure**: Binary radix trie for O(prefix_length) IP matching (max 32 for IPv4, 128 for IPv6)
- **Dynamic updates**: Copy-on-write with atomic swap (no read blocking)
- **Per-IP rate limiting**: Token bucket with configurable connections/duration
- **Memory bounds**: LRU cache with 100,000 max tracked IPs
- **Metrics**: Per-rule hit counters, total accepted/denied

### Connection Rate Limiting

- Sliding window counter per IP within configurable time window
- Bounded memory: LRU cache (100,000 IPs)
- TTL expiration: 2x window duration
- Lock-free via `DashMap` with TTL wrapper
- Close connection on limit exceeded

### Packet Rate Limiting (UDP)

- Global + per-IP token bucket
- Configurable per-IP burst allowance
- Actions: DROP (silent discard), REJECT (close), THROTTLE (queue, future)
- Max 50,000 per-IP entries
- Lazy refill (tokens replenished on acquire, no background timer)

### TLS Fingerprinting (JA3)

- Parse TLS ClientHello to extract fingerprint components
- Components: TLS version, cipher suites, extensions, supported groups, EC point formats
- Filter GREASE values (RFC 8701)
- MD5 hash of fingerprint string
- Thread-safe block list for known-bad fingerprints
- Use for fingerprint-based access control

---

## 18. Module 14: Observability & Metrics

### Prometheus Metrics

Use the `prometheus` crate. Export at `/metrics` endpoint.

**Required Metrics:**
- `expressgateway_connections_active{protocol="tcp|udp|http|h2|h3"}` - Gauge
- `expressgateway_connections_total{protocol}` - Counter
- `expressgateway_requests_total{method, status, protocol}` - Counter
- `expressgateway_request_duration_seconds{method, protocol}` - Histogram
- `expressgateway_bytes_sent_total{direction="upstream|downstream"}` - Counter
- `expressgateway_bytes_received_total{direction}` - Counter
- `expressgateway_backend_health{backend, state}` - Gauge
- `expressgateway_backend_connections_active{backend}` - Gauge
- `expressgateway_controlplane_push_success` - Counter
- `expressgateway_controlplane_push_failure` - Counter
- `expressgateway_controlplane_push_latency_seconds` - Histogram
- `expressgateway_xdp_packets_total{action="tx|redirect|pass|drop"}` - Counter (from BPF per-CPU stats)

### Access Logging

JSON-structured access log per request:

```json
{
    "timestamp": "2024-01-01T00:00:00.000Z",
    "method": "GET",
    "uri": "/api/v1/users",
    "status": 200,
    "latency_ms": 12.5,
    "bytes_sent": 1024,
    "bytes_received": 256,
    "request_id": "uuid-here",
    "protocol": "HTTP/2",
    "tls_version": "TLSv1.3",
    "tls_cipher": "TLS_AES_256_GCM_SHA384",
    "client_ip": "192.168.1.100",
    "backend": "10.0.0.5:8080",
    "user_agent": "Mozilla/5.0..."
}
```

Use separate `tracing` subscriber for access logs (operational separation from application logs).

### Application Logging

Use `tracing` with `tracing-subscriber`:
- JSON format for structured logging
- Environment-based log level filtering (`RUST_LOG`)
- Component-level logging with spans

---

## 19. Module 15: Configuration System

### File-Based Configuration (TOML)

**No ZooKeeper, etcd, or Consul.** Configuration is file-based with hot-reload.

```toml
# /etc/expressgateway/config.toml

[global]
environment = "production"  # "production" or "development"
log_level = "info"
metrics_bind = "0.0.0.0:9090"

[runtime]
backend = "auto"  # "io_uring", "epoll", or "auto"
worker_threads = 0  # 0 = num_cpus
xdp_enabled = true
xdp_interface = "eth0"
xdp_mode = "driver"  # "driver" (DRV_MODE) or "generic" (SKB_MODE)

[transport]
recv_buf_size = 262144
send_buf_size = 262144
tcp_nodelay = true
tcp_quickack = true
tcp_keepalive = true
tcp_fastopen = true
tcp_fastopen_queue = 256
so_backlog = 50000
so_reuseport = true
connect_timeout_ms = 10000

[buffer]
page_size = 16384
max_order = 11
small_cache_size = 256
normal_cache_size = 64

[tls]
enabled = true
profile = "intermediate"  # "modern" or "intermediate"
handshake_timeout_ms = 10000
session_timeout_s = 43200
session_cache_size = 1000000

[tls.server]
default_cert = "/etc/expressgateway/certs/default.pem"
default_key = "/etc/expressgateway/certs/default.key"
mutual_tls = "not_required"  # "not_required", "optional", "required"
trust_ca = "/etc/expressgateway/certs/ca.pem"
crl_file = ""  # Optional CRL path

[tls.client]
verify_hostname = true
session_timeout_s = 300

[[tls.sni_certs]]
hostname = "example.com"
cert = "/etc/expressgateway/certs/example.pem"
key = "/etc/expressgateway/certs/example.key"
ocsp_stapling = true

[[tls.sni_certs]]
hostname = "*.example.com"
cert = "/etc/expressgateway/certs/wildcard.pem"
key = "/etc/expressgateway/certs/wildcard.key"

[[listeners]]
name = "http"
protocol = "http"  # "tcp", "udp", "http", "https", "h2c", "quic"
bind = "0.0.0.0:80"

[[listeners]]
name = "https"
protocol = "https"
bind = "0.0.0.0:443"
tls_profile = "intermediate"
http_versions = ["h2", "http/1.1"]

[[listeners]]
name = "quic"
protocol = "quic"
bind = "0.0.0.0:443"
alt_svc_max_age = 3600

[[listeners]]
name = "tcp-passthrough"
protocol = "tcp"
bind = "0.0.0.0:3306"
xdp_accelerated = true  # Use XDP for this listener

[[clusters]]
name = "web-backend"
lb_strategy = "round_robin"  # "round_robin", "weighted_round_robin", "least_connection", "least_load", "random", "consistent_hash"
max_connections_per_node = 10000
drain_timeout_s = 30

[[clusters.nodes]]
address = "10.0.0.1:8080"
weight = 5
max_connections = 10000

[[clusters.nodes]]
address = "10.0.0.2:8080"
weight = 3

[[clusters]]
name = "grpc-backend"
lb_strategy = "least_connection"

[[clusters.nodes]]
address = "10.0.0.10:50051"

[[routes]]
host = "example.com"
path = "/api/*"
cluster = "web-backend"
priority = 10
lb_strategy = "http_least_response_time"

[[routes]]
host = "*.example.com"
path = "/grpc/*"
cluster = "grpc-backend"
priority = 20

[http]
max_request_body_size = 10485760  # 10 MB
max_response_body_size = 268435456  # 256 MB
max_connection_body_size = 268435456  # 256 MB (H2 aggregate)
max_header_size = 8192
max_uri_length = 8192
request_header_timeout_ms = 5000
request_body_timeout_ms = 30000
idle_timeout_s = 120
keepalive_timeout_s = 60
max_concurrent_streams = 100  # H2
h2_connection_window_size = 1048576  # 1 MB

[http.compression]
enabled = true
min_size = 256  # Minimum response size to compress
algorithms = ["br", "zstd", "gzip", "deflate"]

[http.sticky_session]
enabled = false
cookie_name = "X-SBZ-EGW-RouteID"

[proxy_protocol]
inbound = "off"   # "off", "v1", "v2", "auto"
outbound = "off"  # "off", "v1", "v2"

[health_check]
interval_s = 10
timeout_ms = 5000
rise = 2
fall = 3
samples = 100

[health_check.http]
method = "GET"
path = "/health"
expected_status = [200]

[circuit_breaker]
failure_threshold = 5
success_threshold = 3
open_duration_s = 30

[security]
mode = "denylist"  # "allowlist" or "denylist"
max_tracked_ips = 100000

[security.connection_rate_limit]
max_per_ip = 100
window_s = 60

[security.packet_rate_limit]
global_pps = 1000000
per_ip_pps = 10000
per_ip_burst = 1000
action = "drop"  # "drop", "reject"

[controlplane]
enabled = false
grpc_bind = "0.0.0.0:9443"
rest_bind = "0.0.0.0:8443"
heartbeat_interval_s = 10
heartbeat_miss_threshold = 3
heartbeat_disconnect_threshold = 6

[graceful_shutdown]
drain_timeout_s = 30
```

### Hot-Reload

- Watch config file via `inotify` (Linux) / `kqueue` (macOS/BSD)
- On file change: parse new config, validate, atomic swap
- Validation MUST pass before apply (fail-fast, keep running config)
- Log detailed diff of what changed
- SIGHUP also triggers reload

### Config Validation

- All ports in valid range (1-65535)
- All bind addresses parseable
- Referenced clusters exist for routes
- TLS cert/key files exist and are parseable
- Weights are positive integers
- Timeouts are positive
- No duplicate listener binds

---

## 20. Module 16: Control Plane

### Architecture

The control plane is a separate process that manages multiple data plane instances. It provides:
- REST API (axum) for human/automation interaction
- gRPC (tonic) for data plane node communication

### REST API Endpoints

Base path: `/api/v1/controlplane/`

| Method | Path | Description |
|--------|------|-------------|
| GET | `/nodes` | List connected nodes |
| GET | `/nodes/{id}` | Get specific node |
| POST | `/nodes/{id}/drain` | Drain node |
| POST | `/nodes/{id}/undrain` | Resume node |
| DELETE | `/nodes/{id}` | Force remove |
| GET | `/status` | Overall CP status |
| GET | `/status/nodes` | Node state breakdown |
| GET | `/config/versions` | List config versions |
| GET | `/config/current` | Current revision |
| POST | `/config/rollback` | Rollback to version |
| POST | `/clusters` | Create cluster |
| GET | `/clusters` | List clusters |
| GET | `/clusters/{id}` | Get cluster |
| PUT | `/clusters/{id}` | Update cluster |
| DELETE | `/clusters/{id}` | Delete cluster |
| POST | `/routes` | Create route |
| GET | `/routes` | List routes |
| PUT | `/routes/{id}` | Update route |
| DELETE | `/routes/{id}` | Delete route |
| POST | `/listeners` | Create listener |
| GET | `/listeners` | List listeners |
| PUT | `/listeners/{id}` | Update listener |
| DELETE | `/listeners/{id` | Delete listener |
| POST | `/health-checks` | Create health check |
| GET | `/health-checks` | List health checks |
| PUT | `/health-checks/{id}` | Update |
| DELETE | `/health-checks/{id}` | Delete |
| POST | `/tls-certs` | Create TLS cert |
| GET | `/tls-certs` | List TLS certs |
| PUT | `/tls-certs/{id}` | Update |
| DELETE | `/tls-certs/{id}` | Delete |

### gRPC Services

Define protobuf services matching the Java implementation:

1. **NodeRegistrationService**: Register, Deregister, Heartbeat (bidirectional stream)
2. **ConfigDistributionService**: StreamConfig (ADS pattern), FetchConfig (pull)
3. **NodeControlService**: SendCommand, StreamCommands (bidirectional)

### Authentication

- Node registers with `auth_token`, receives `session_token` (UUID)
- Session token required in `x-session-token` header for all subsequent RPCs
- Rate limiting per node: 100 requests/sec default

### Node Lifecycle

```
Register -> CONNECTED -> (miss heartbeats) -> UNHEALTHY -> (more misses) -> DISCONNECTED
                |                                                                |
                +-- Drain command -> DRAINING -> (connections drained) -----------+
```

### Config Distribution

1. Mutation submitted via REST API
2. Config validated and versioned
3. Delta computed for each connected node
4. Pushed via gRPC ConfigDistributionService
5. Node ACKs or NACKs
6. Metrics recorded

### `[DEFERRED]` HA Features

The following HA features from the Java implementation will be implemented later:
- Leader election across CP instances
- Write forwarding from followers to leader
- Cross-region replication
- Partition detection and quorum handling
- Canary deployment fan-out

For now, the control plane runs as a **single instance** with file-based persistence.

---

## 21. Module 17: Connection Management & Pooling

### HTTP/1.1 Connection Pool

- **LIFO ordering**: Last-in-first-out for warm connection reuse
- Per-node pool via `VecDeque`
- Max connections per node: configurable (`max_h1_per_node`)
- Idle eviction: sweep every 30 seconds, evict connections idle > `pool_idle_timeout`
- Max connection age: evict connections older than `max_connection_age` (default 5 minutes)
- Pool hit/miss/eviction metrics

### HTTP/2 Connection Pool

- **Shared connections**: Multiple streams per connection
- Per-node pool via `Vec`
- Stream capacity check: `active_streams < max_concurrent_streams`
- Max connections per node: `max_h2_per_node`
- New connection created if all at capacity

### QUIC Connection Pool

- Connection ID routing for multiplexing
- Shared QUIC connections across streams
- Per-flow state tracking (DCID, SCID)

### TCP Connection Pool (L4)

- **FIFO ordering**: Oldest connections reused first (detect stale quickest)
- Validation on acquire: check connection usability
- Dead connections silently discarded
- Per-backend: 8 idle connections max (default)
- Total: 256 pooled connections (default)
- Idle timeout: 60 seconds

### Shared Eviction Executor

- Single background task for all ConnectionPool instances (prevent thread explosion)
- 30-second sweep interval

---

## 22. Module 18: Compression

### Supported Algorithms

1. **Brotli** (`br`): Highest compression, use `brotli` crate
2. **Zstandard** (`zstd`): Good compression + speed, use `zstd` crate
3. **Gzip** (`gzip`): Standard, use `flate2` crate
4. **Deflate** (`deflate`): Legacy, use `flate2` crate

### Accept-Encoding Negotiation

- Parse quality values: `Accept-Encoding: br;q=1.0, gzip;q=0.8, *;q=0.1`
- Preference order (when equal quality): br > zstd > gzip > deflate
- Wildcard support: `*` accepts any encoding
- Skip compression if:
  - Content-Encoding already present
  - Content-Length below threshold (`compression_threshold`)
  - Content-Type not in compressible list

### Compressible MIME Types

text/html, text/css, text/plain, text/javascript, text/xml, application/json, application/javascript, application/xml, application/xhtml+xml, application/rss+xml, application/atom+xml, application/svg+xml, application/wasm, image/svg+xml, font/ttf, font/otf, font/woff, font/woff2, application/vnd.ms-fontobject, application/x-font-ttf, application/x-font-opentype, application/font-woff, application/font-woff2, application/octet-stream (for specific subresources), multipart/bag, multipart/mixed

---

## 23. Module 19: Proxy Protocol

### HAProxy PROXY Protocol v1 (Text)

**Decode (Inbound):**
- Format: `PROXY TCP4|TCP6|UNKNOWN <srcIP> <dstIP> <srcPort> <dstPort>\r\n`
- Max 108 bytes
- Parse protocol family, addresses, ports
- Store real client address for downstream use

**Encode (Outbound):**
- Generate text format with protocol family validation
- Both addresses must be same family (IPv4 or IPv6), else UNKNOWN

### HAProxy PROXY Protocol v2 (Binary)

**Decode (Inbound):**
- 12-byte signature: `\r\n\r\n\0\r\nQUIT\n`
- Version/command byte, address family, transport protocol
- IPv4: 12 bytes address data
- IPv6: 36 bytes address data
- LOCAL command: no address data

**Encode (Outbound):**
- Binary format with proper address family codes
- Fall back to LOCAL command if addresses unavailable

### Shared Semantics

- **One-shot**: Remove handler from pipeline after first message
- **Real address priority**: Use PROXY protocol address if available, fall back to TCP peer
- **IPv4-mapped IPv6**: Normalize `::ffff:x.x.x.x` to IPv4
- **Ordering**: PROXY protocol header MUST be sent before any application data (before TLS ClientHello if TLS enabled on backend)

---

## 24. Module 20: Bootstrap & Lifecycle

### Startup Sequence

1. Parse command-line arguments (config file path, log level overrides)
2. Load and validate configuration from TOML file
3. Initialize tracing/logging
4. Initialize metrics registry
5. Create runtime (io_uring or epoll)
6. If XDP enabled: load and attach XDP programs
7. Initialize TLS contexts (load certificates)
8. Initialize health checkers for all backends
9. Create backend clusters with load balancers
10. Start health check scheduler
11. Bind listeners (TCP, UDP, HTTP, HTTPS, QUIC)
12. If control plane enabled: start gRPC + REST servers
13. Install signal handlers (SIGTERM, SIGINT, SIGHUP)
14. Log "ExpressGateway started" with version, listeners, backends

### Graceful Shutdown (SIGTERM / SIGINT)

1. Stop accepting new connections
2. If XDP: detach XDP programs
3. Send GOAWAY to HTTP/2 connections
4. Send GOAWAY to HTTP/3 connections
5. Send Close frames to WebSocket connections
6. Wait for in-flight requests (drain timeout, default 30s)
7. Close all backend connections
8. Stop health checkers
9. Stop control plane
10. Flush metrics
11. Flush access logs
12. Exit cleanly

### Hot Config Reload (SIGHUP)

1. Re-read config file
2. Validate new config
3. If valid: atomic swap, apply changes
4. If invalid: log error, keep running config
5. Changes that can be hot-reloaded:
   - Backend additions/removals
   - TLS certificate updates
   - Route changes
   - Rate limit adjustments
   - Health check configuration
6. Changes that require restart:
   - Listener bind addresses
   - Runtime backend (io_uring/epoll)
   - XDP interface

### System Tuning (initialize.sh equivalent)

Document recommended sysctl settings:
```bash
# File descriptor limits
fs.file-max = 20000000
# Connection tracking
net.netfilter.nf_conntrack_max = 2000000
# TCP tuning
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
# UDP tuning
net.core.rmem_default = 262144
net.core.wmem_default = 262144
```

---

## 25. Testing Strategy

### Unit Tests

Every public function MUST have unit tests. Use `#[cfg(test)]` modules.

### Integration Tests

Located in `tests/integration/`:
- TCP proxy end-to-end
- UDP proxy end-to-end
- HTTP/1.1 proxy with all features
- HTTP/2 proxy with stream multiplexing
- HTTP/3 proxy over QUIC
- gRPC proxy with all streaming types
- WebSocket proxy with upgrade
- TLS termination and mTLS
- PROXY protocol v1 and v2
- Load balancing algorithm correctness
- Health check state transitions
- Circuit breaker state machine
- Configuration hot-reload
- Graceful shutdown
- Backpressure under load

### Security Tests

- Request smuggling (CL/TE desync, negative Content-Length)
- CRLF injection in HTTP/2
- Control frame flood (SETTINGS + PING)
- Slowloris (slow headers)
- Slow POST (slow body)
- Path traversal (`/../`)
- Null byte injection (`%00`)
- TLS downgrade attempts
- Invalid certificate rejection

### Performance Tests

Use `criterion` for benchmarks:
- Load balancing algorithm throughput (ops/sec)
- HTTP/1.1 request throughput
- HTTP/2 stream throughput
- Connection pool acquire/release
- Hash ring lookup latency
- EWMA update latency

### Chaos Tests

- Backend failure during request
- Config file corruption
- Certificate expiry during operation
- Network partition simulation
- Memory pressure with backlog limits
- Reconnect storms

### Test Coverage

Target: **>80% line coverage** for all crates (matching Java's JaCoCo requirement). Use `cargo-llvm-cov`.

---

## 26. Performance Requirements

### Throughput Targets

| Metric | Target | Measurement |
|--------|--------|-------------|
| L4 TCP (XDP) | >40 Mpps | 64-byte packets, single core |
| L4 UDP (XDP) | >40 Mpps | 64-byte packets, single core |
| HTTP/1.1 | >500K req/s | Keep-alive, 1KB response |
| HTTP/2 | >500K req/s | Multiplexed streams, 1KB response |
| HTTP/3 | >200K req/s | QUIC, 1KB response |
| TLS handshakes | >50K/s | TLS 1.3, ECDHE P-256 |

### Latency Targets

| Metric | p50 | p99 | p999 |
|--------|-----|-----|------|
| L4 TCP (XDP) | <1us | <5us | <10us |
| HTTP/1.1 proxy | <50us | <200us | <1ms |
| HTTP/2 proxy | <50us | <200us | <1ms |
| TLS handshake | <1ms | <5ms | <10ms |

### Memory Targets

- Idle: <50 MB RSS
- Per connection: <10 KB overhead
- No unbounded growth under sustained load
- Session persistence maps bounded (see max_entries in Section 9)

---

## 27. Quality Gates

Every PR/merge MUST pass these gates:

1. **Compilation**: `cargo build --release` succeeds with zero warnings
2. **Tests**: `cargo test` passes 100%
3. **Clippy**: `cargo clippy -- -D warnings` passes
4. **Format**: `cargo fmt --check` passes
5. **Coverage**: >80% line coverage per crate
6. **No unsafe**: Minimize `unsafe` blocks. Every `unsafe` block must have a `// SAFETY:` comment explaining why it's sound. XDP/BPF code is exempt.
7. **No panics in production paths**: Use `Result<T, E>` everywhere. `unwrap()` and `expect()` only in tests and initialization.
8. **Documentation**: All public types and functions have doc comments
9. **Benchmarks**: No regression >10% from baseline
10. **Security audit**: No OWASP Top 10 vulnerabilities
11. **RFC compliance**: Verified against relevant RFCs

---

## 28. Reference: Complete Feature Parity Checklist

This is the definitive checklist. Every item MUST be checked off before the rewrite is considered complete.

### L4 - TCP
- [ ] TCP proxy with connection tracking
- [ ] XDP-accelerated TCP forwarding
- [ ] TCP connection pooling (FIFO, 8 idle/backend, 256 total, 60s timeout)
- [ ] Half-close support (RFC 9293)
- [ ] Connection backlog queue (10,000 max, memory-pressure adaptive)
- [ ] Connection reset propagation (SO_LINGER=0 on RST)
- [ ] Graceful drain with timeout (default 30s)
- [ ] TCP Fast Open (client and server)
- [ ] TCP_NODELAY, TCP_QUICKACK, SO_KEEPALIVE
- [ ] SO_REUSEPORT for multi-core scaling
- [ ] Backpressure (write buffer water marks: 32KB/64KB)

### L4 - UDP
- [ ] UDP proxy with session management
- [ ] XDP-accelerated UDP forwarding
- [ ] Session expiry (30s default) via expiring map
- [ ] Session rate limiting (token bucket, per-source-IP)
- [ ] UDP GRO (Generic Receive Offload)

### Load Balancing - L4
- [ ] Round Robin (lock-free atomic index)
- [ ] Weighted Round Robin (NGINX-style smooth)
- [ ] Least Connection
- [ ] Least Load (connections + load percentage)
- [ ] Random (thread-local RNG)
- [ ] Consistent Hash (BTreeMap, 150 vnodes, Murmur3-128, RwLock)

### Session Persistence - L4
- [ ] Source IP Hash (/24 IPv4, /48 IPv6, 1h TTL, 100K max)
- [ ] Four-Tuple Hash (1h TTL, 500K max)
- [ ] Consistent Hash Persistence (stateless, ring-based)

### Load Balancing - L7
- [ ] HTTP Round Robin
- [ ] HTTP Consistent Hash (configurable key: header or IP)
- [ ] HTTP Random
- [ ] HTTP Least Response Time (EWMA, cold-start, alpha=0.5)
- [ ] Sticky Session (cookie X-SBZ-EGW-RouteID, SHA-256, HttpOnly, SameSite=Strict, Secure)

### TLS / Cryptography
- [ ] TLS 1.2 support
- [ ] TLS 1.3 support
- [ ] TLS 1.1 rejection in production
- [ ] Modern cipher profile (3 ciphers)
- [ ] Intermediate cipher profile (7 ciphers)
- [ ] SNI-based certificate selection (exact + wildcard)
- [ ] Default certificate fallback
- [ ] ALPN negotiation (h2, http/1.1)
- [ ] OCSP stapling (6h refresh, 16-byte nonce, signature verification)
- [ ] Mutual TLS (NOT_REQUIRED, OPTIONAL, REQUIRED)
- [ ] CRL checking
- [ ] Certificate hot-reload (RwLock atomic swap)
- [ ] 30-day expiration warning
- [ ] DSA key rejection
- [ ] TLS handshake timeout (10s)
- [ ] Accept-all-certs rejection in production

### HTTP/1.1
- [ ] Request/response proxying
- [ ] Keep-alive with connection pooling
- [ ] Chunked transfer encoding
- [ ] Pipelining (serial response ordering)
- [ ] 100-continue handling
- [ ] Hop-by-hop header stripping (RFC 9110 Section 7.6.1)
- [ ] Header injection (Via, X-Request-ID, X-Forwarded-For/Proto)
- [ ] Request body size limit (413)
- [ ] Request header timeout (Slowloris defense)
- [ ] Request body timeout (slow-POST defense)
- [ ] URI normalization (RFC 3986)
- [ ] Path traversal prevention
- [ ] Null byte rejection
- [ ] Health endpoints (/health, /ready)
- [ ] Error responses (400, 413, 414, 431, 502, 503, 504)

### HTTP/2
- [ ] Stream multiplexing
- [ ] Per-stream load balancing
- [ ] HPACK header compression
- [ ] Flow control (connection + stream level)
- [ ] h2c (cleartext, prior knowledge detection)
- [ ] CONNECT tunneling (RFC 9113 Section 8.5)
- [ ] Control frame flood protection (100/10s, ENHANCE_YOUR_CALM)
- [ ] Per-stream body size limit
- [ ] Per-connection aggregate body size limit (256 MB)
- [ ] GOAWAY graceful shutdown
- [ ] H1 <-> H2 protocol translation (both directions)
- [ ] Backpressure with multi-backend aggregation

### HTTP/3 / QUIC
- [ ] QUIC transport (quinn)
- [ ] HTTP/3 request/response
- [ ] QUIC connection pooling
- [ ] Stateless retry with token validation
- [ ] Connection migration
- [ ] NAT rebinding detection
- [ ] Path validation
- [ ] 0-RTT with replay protection
- [ ] Alt-Svc header injection (RFC 7838)
- [ ] H1/H2 <-> H3 protocol translation
- [ ] Connection ID routing
- [ ] QUIC flow control

### gRPC
- [ ] gRPC detection (Content-Type: application/grpc)
- [ ] HTTP -> gRPC status code mapping (14 mappings)
- [ ] Deadline parsing (grpc-timeout header)
- [ ] Deadline enforcement (max 300s clamp)
- [ ] Unary streaming
- [ ] Server streaming
- [ ] Client streaming
- [ ] Bidirectional streaming
- [ ] Trailer forwarding (grpc-status, grpc-message)
- [ ] Health check endpoint (/grpc.health.v1.Health/Check)
- [ ] gRPC-aware compression

### WebSocket
- [ ] HTTP/1.1 upgrade
- [ ] Frame handling (Text, Binary, Ping, Pong, Close, Continuation)
- [ ] Local Ping/Pong handling (prevent duplicate Pongs)
- [ ] Max frame size (64 KB default)
- [ ] Message Too Big (1009)
- [ ] WebSocket-over-HTTP/2 (RFC 8441)
- [ ] Bidirectional backpressure
- [ ] Idle timeout with Close frame (1001)

### Health Checking
- [ ] TCP health check (connect-based)
- [ ] UDP health check (PING/PONG)
- [ ] HTTP health check (configurable method/path/status)
- [ ] Rise/fall thresholds (HAProxy-style)
- [ ] Health states (GOOD >95%, MEDIUM 75-95%, BAD <75%, UNKNOWN)
- [ ] Health cache (5s TTL)
- [ ] Exponential backoff (1s base, 60s max)
- [ ] Health chck scheduler

### Circuit Breaker
- [ ] CLOSED -> OPEN (failure threshold)
- [ ] OPEN -> HALF_OPEN (duration elapsed)
- [ ] HALF_OPEN -> CLOSED (success threshold)
- [ ] HALF_OPEN -> OPEN (any failure)
- [ ] Lock-free CAS transitions
- [ ] Limited concurrent requests in HALF_OPEN

### Outlier Detection
- [ ] Traffic-based success/failure monitoring
- [ ] Consecutive failure counter per node
- [ ] Automatic ejection (to OFFLINE)
- [ ] Automatic restoration (to IDLE) after ejection window
- [ ] Max ejection percentage cap

### Security
- [ ] NACL (radix trie, allowlist/denylist, O(prefix_length))
- [ ] Connection rate limiting (per-IP, token bucket, LRU 100K)
- [ ] Packet rate limiting (global + per-IP, UDP)
- [ ] TLS fingerprinting (JA3, GREASE filtering, block list)
- [ ] IP allowlisting/blocklisting with CIDR

### Compression
- [ ] Brotli
- [ ] Gzip
- [ ] Zstandard
- [ ] Deflate
- [ ] Accept-Encoding negotiation with quality values
- [ ] Compressible MIME type whitelist
- [ ] Compression threshold (skip small responses)

### Proxy Protocol
- [ ] v1 decode (text, max 108 bytes)
- [ ] v1 encode
- [ ] v2 decode (binary, IPv4/IPv6/LOCAL)
- [ ] v2 encode
- [ ] Auto-detect (v1 vs v2)
- [ ] One-shot semantics (remove after first message)
- [ ] IPv4-mapped IPv6 normalization
- [ ] Real address propagation to downstream handlers

### Connection Management
- [ ] H1 pool (LIFO, idle eviction, max age)
- [ ] H2 pool (shared, stream-counted)
- [ ] QUIC pool (CID routing)
- [ ] TCP pool (FIFO, validation on acquire)
- [ ] Shared eviction executor (single background task)
- [ ] Per-node O(1) connection count (atomic)
- [ ] Max connections per node (default 10,000)
- [ ] TOCTOU-safe connection admission

### Node Management
- [ ] States: ONLINE, OFFLINE, IDLE, MANUAL_OFFLINE
- [ ] Thread-safe state transitions
- [ ] Bytes sent/received tracking (atomic)
- [ ] Connection draining (HTTP/2 GOAWAY, 5s grace)
- [ ] Health check integration per node

### Observability
- [ ] Prometheus metrics export (/metrics)
- [ ] JSON structured access logs
- [ ] Per-request latency tracking
- [ ] XDP per-CPU statistics
- [ ] Backend health state gauges
- [ ] Control plane push metrics

### Configuration
- [ ] TOML file-based configuration
- [ ] Hot-reload via inotify + SIGHUP
- [ ] Atomic config swaps (validate before apply)
- [ ] Config validation (fail-fast)

### Control Plane
- [ ] REST API (15+ CRUD endpoints)
- [ ] gRPC services (NodeRegistration, ConfigDistribution, NodeControl)
- [ ] Session token authentication
- [ ] Heartbeat tracking (CONNECTED -> UNHEALTHY -> DISCONNECTED)
- [ ] Graceful drain with RECONNECT
- [ ] Config versioning and rollback
- [ ] Delta sync distribution
- [ ] Reconnect storm protection (token bucket)
- [ ] Node registry with state machine
- [ ] Rate limiting per node (100 req/s)

### Bootstrap & Lifecycle
- [ ] Deterministic startup sequence
- [ ] Graceful shutdown (SIGTERM/SIGINT)
- [ ] Hot config reload (SIGHUP)
- [ ] XDP attach/detach lifecycle
- [ ] Connection drain on shutdown
- [ ] Signal handling

---

## Appendix A: Key Design Decisions

### Why XDP for L4 and not for L7

L4 proxying (TCP/UDP passthrough without payload inspection) is a pure packet-forwarding operation. The kernel already has all the information needed (IP headers, TCP/UDP ports) to make a forwarding decision. XDP runs at the NIC driver level before the kernel networking stack, eliminating all kernel overhead. For L7, we need to parse HTTP headers, manage TLS state, track streams, apply routing rules -- this requires userspace logic that XDP cannot express within eBPF verifier constraints.

### Why io_uring over epoll

io_uring provides:
- Batched syscall submission (one syscall per batch vs one per operation)
- Kernel-side SQ polling (optional, eliminates all syscalls)
- Zero-copy I/O via registered buffers
- Splice support for zero-copy proxying between sockets
- Multishot accept (one submission, multiple accepts)

The epoll fallback ensures the system runs on older kernels.

### Why rustls over OpenSSL

- Pure Rust: no C dependencies, no build complexity
- Memory safety: no use-after-free bugs in TLS handling
- Modern defaults: TLS 1.3 preferred, no legacy cipher support by default
- Faster than OpenSSL for TLS 1.3 handshakes in benchmarks
- ring crypto backend: audited, constant-time implementations

### Why TOML over YAML/JSON

- Human-readable and writable (primary use case: manual editing)
- Strong typing (strings vs integers vs booleans are unambiguous)
- Comment support (unlike JSON)
- Less error-prone than YAML (no implicit type coercion, no significant whitespace)
- First-class Rust support via `toml` crate + `serde`

### Why No ZooKeeper/etcd/Consul

The user explicitly requested simple file-based configuration. The existing Java implementation's coordination layer (ZooKeeper, etcd, Consul) adds significant operational complexity. The control plane will handle config distribution with HA features in a later phase, using Raft consensus built into the Rust binary rather than external dependencies.

---

## Appendix B: Crate Dependency Graph

```
eg-core (no deps)
    ├── eg-config (eg-core, toml, serde)
    ├── eg-runtime (eg-core, io-uring, tokio)
    ├── eg-tls (eg-core, eg-config, rustls)
    ├── eg-lb (eg-core)
    ├── eg-health (eg-core, eg-lb)
    ├── eg-pool (eg-core, eg-lb)
    ├── eg-security (eg-core, eg-config)
    ├── eg-compression (eg-core)
    ├── eg-proxy-protocol (eg-core, bytes)
    ├── eg-metrics (eg-core, prometheus)
    ├── eg-xdp (eg-core, eg-config, aya)
    ├── eg-protocol-tcp (eg-core, eg-runtime, eg-tls, eg-lb, eg-pool, eg-security, eg-proxy-protocol, eg-metrics)
    ├── eg-protocol-udp (eg-core, eg-runtime, eg-lb, eg-security, eg-metrics)
    ├── eg-protocol-http (eg-core, eg-runtime, eg-tls, eg-lb, eg-pool, eg-compression, eg-security, eg-proxy-protocol, eg-metrics, hyper, h2)
    ├── eg-protocol-h3 (eg-core, eg-runtime, eg-tls, eg-lb, eg-pool, eg-metrics, quinn, h3)
    ├── eg-protocol-grpc (eg-core, eg-protocol-http, prost)
    ├── eg-protocol-ws (eg-core, eg-protocol-http, tokio-tungstenite)
    ├── eg-controlplane (eg-core, eg-config, eg-lb, eg-health, eg-metrics, axum, tonic, prost)
    ├── eg-agent (eg-core, eg-config, tonic, prost)
    └── eg-bootstrap (ALL crates)
```

---

## Appendix C: Reference RFCs

| RFC | Topic | Relevant Modules |
|-----|-------|-----------------|
| RFC 9110 | HTTP Semantics | eg-protocol-http |
| RFC 9112 | HTTP/1.1 | eg-protocol-http |
| RFC 9113 | HTTP/2 | eg-protocol-http |
| RFC 9114 | HTTP/3 | eg-protocol-h3 |
| RFC 9000 | QUIC Transport | eg-protocol-h3 |
| RFC 9001 | QUIC TLS | eg-tls, eg-protocol-h3 |
| RFC 6455 | WebSocket | eg-protocol-ws |
| RFC 8441 | WebSocket over HTTP/2 | eg-protocol-ws |
| RFC 7301 | ALPN | eg-tls |
| RFC 7838 | Alt-Svc | eg-protocol-h3 |
| RFC 6265 | Cookies | eg-lb (sticky session) |
| RFC 8701 | GREASE | eg-security (JA3) |
| RFC 3986 | URI | eg-protocol-http |
| RFC 8954 | OCSP Nonce | eg-tls |
| RFC 6960 | OCSP | eg-tls |
| RFC 8996 | TLS 1.0/1.1 Deprecation | eg-tls |
| RFC 9293 | TCP | eg-protocol-tcp |
| RFC 1624 | Incremental Checksum | eg-xdp |
| HAProxy PROXY Protocol v1/v2 | Proxy Protocol | eg-proxy-protocol |

---

*This document was generated by analyzing the complete ShieldBlaze ExpressGateway Java codebase (28 Maven modules, 350+ source files, 240+ test classes) and mapping every feature to its Rust equivalent. It serves as the single source of truth for the rewrite.*
