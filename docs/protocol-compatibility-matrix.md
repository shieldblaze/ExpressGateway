# Protocol Compatibility Matrix

## HTTP Version Combinations (L7)

All 9 frontend ↔ backend combinations are supported:

| Frontend | Backend | Status | Crate | Notes |
|----------|---------|--------|-------|-------|
| HTTP/1.1 | HTTP/1.1 | Implemented | protocol-http | Direct forwarding, keep-alive, pipelining |
| HTTP/1.1 | HTTP/2 | Implemented | protocol-http | H1 request → H2 stream, pseudo-header injection |
| HTTP/1.1 | HTTP/3 | Implemented | protocol-http + protocol-h3 | H1 → QUIC stream, header case normalization |
| HTTP/2 | HTTP/1.1 | Implemented | protocol-http | H2 stream → H1 request, pseudo-header extraction |
| HTTP/2 | HTTP/2 | Implemented | protocol-http | Stream-to-stream, flow control preserved |
| HTTP/2 | HTTP/3 | Implemented | protocol-http + protocol-h3 | H2 ↔ H3 flow control mapping |
| HTTP/3 | HTTP/1.1 | Implemented | protocol-h3 | QUIC stream → H1, header translation |
| HTTP/3 | HTTP/2 | Implemented | protocol-h3 | H3 ↔ H2 stream mapping |
| HTTP/3 | HTTP/3 | Implemented | protocol-h3 | QUIC stream-to-stream |

### Protocol Translation Details

**Header handling across versions:**
- H1 → H2/H3: Request line decomposed into `:method`, `:path`, `:scheme`, `:authority` pseudo-headers
- H2/H3 → H1: Pseudo-headers reconstructed into request line
- Header case: H1 preserves original case, H2/H3 lowercases all
- Hop-by-hop headers stripped at version boundaries (Connection, Keep-Alive, Proxy-Connection, TE [except trailers], Transfer-Encoding, Upgrade)
- `TE: trailers` preserved for gRPC compatibility (RFC 9113 Section 8.2.2)
- `Proxy-Authorization` and `Proxy-Authenticate` correctly preserved (end-to-end headers)

**Body encoding:**
- H1 chunked ↔ H2/H3 DATA frames: automatic translation
- Content-Length preserved when known; chunked encoding used otherwise

**Flow control:**
- H2 ↔ H3: Per-stream flow control windows mapped across the proxy
- Backpressure propagation: no unbounded buffering at any boundary

## gRPC Combinations

| Frontend | Backend | Status | Notes |
|----------|---------|--------|-------|
| gRPC/H2 | gRPC/H2 | Implemented | Native pass-through |
| gRPC/H2 | gRPC/H3 | Implemented | H2 stream → QUIC stream |
| gRPC/H3 | gRPC/H2 | Implemented | QUIC stream → H2 stream |
| gRPC/H3 | gRPC/H3 | Implemented | QUIC-to-QUIC |
| gRPC-Web/H1 | gRPC/H2 | Implemented | gRPC-Web translation layer |

### gRPC Semantics Preserved
- Streaming: client-streaming, server-streaming, bidirectional
- Trailers: `grpc-status`, `grpc-message`, `grpc-status-details-bin`
- Binary framing: 1-byte compressed flag + 4-byte length + data (max 4MB default)
- `te: trailers` forwarded correctly
- Deadline propagation via `grpc-timeout` header (8-digit limit, unit selection)
- Compression negotiation: `grpc-encoding`, `grpc-accept-encoding`
- Health protocol: `grpc.health.v1.Health/Check` and `Watch`
- RST_STREAM → gRPC status mapping (REFUSED_STREAM→UNAVAILABLE, CANCEL→CANCELLED, etc.)
- gRPC-Web: trailer frame encoding (0x80 flag), content-type translation

### gRPC Status Codes
All 17 gRPC status codes implemented:
OK, CANCELLED, UNKNOWN, INVALID_ARGUMENT, DEADLINE_EXCEEDED, NOT_FOUND,
ALREADY_EXISTS, PERMISSION_DENIED, RESOURCE_EXHAUSTED, FAILED_PRECONDITION,
ABORTED, OUT_OF_RANGE, UNIMPLEMENTED, INTERNAL, UNAVAILABLE, DATA_LOSS,
UNAUTHENTICATED

## WebSocket Combinations

| Frontend | Backend | Status | Notes |
|----------|---------|--------|-------|
| WS over H1 Upgrade | WS over H1 | Implemented | RFC 6455 Upgrade |
| WS over H2 CONNECT | WS over H1 | Implemented | RFC 8441 → RFC 6455 downgrade |
| WS over H3 CONNECT | WS over H1 | Implemented | H3 Extended CONNECT → RFC 6455 |
| WS over H2 CONNECT | WS over H2 | Implemented | Extended CONNECT both sides |
| WS over H3 CONNECT | WS over H3 | Implemented | Extended CONNECT both sides |

### WebSocket Features
- Frame-level proxying: text, binary, ping, pong, close
- Backpressure-aware frame forwarding
- Close handshake: close frame + response with proper status codes
- Ping/pong passthrough and keepalive tracking
- Subprotocol negotiation forwarding
- Origin validation against allowed list
- Maximum frame size enforcement
- Handshake timeout, idle timeout, close handshake timeout

## Transport Layer (L4)

| Protocol | Status | Notes |
|----------|--------|-------|
| TCP ↔ TCP | Implemented | Zero-copy capable (64KB buffers), XDP fast-path |
| UDP ↔ UDP | Implemented | Session tracking with timeout eviction |
| QUIC termination | Implemented | Via quinn crate |
| QUIC passthrough | Implemented | XDP connection-ID steering |
| QUIC ↔ TCP bridge | Scaffolded | Protocol translation points defined |

## PROXY Protocol

| Version | Direction | Status |
|---------|-----------|--------|
| v1 (text) | Inbound | Implemented |
| v1 (text) | Outbound | Implemented |
| v2 (binary) | Inbound | Implemented |
| v2 (binary) | Outbound | Implemented |
| Auto-detect | Inbound | Implemented |

### PROXY Protocol Features
- Address families: TCP4, TCP6, UDP4, UDP6, UNIX Stream, UNIX Dgram
- TLV extensions: ALPN, SSL, Authority, CRC32C, NOOP, Unique ID, Netns
- Zero-copy v1 encoding (stack-allocated buffer)
- Zero-allocation v1 parsing (iterator-based field scan)

## TLS / ALPN

| Feature | Status |
|---------|--------|
| TLS 1.2 | Supported |
| TLS 1.3 | Supported |
| ALPN negotiation | Implemented |
| SNI-based routing | Implemented |
| SNI passthrough (no termination) | Implemented |
| mTLS (optional/required) | Implemented |
| Certificate hot-reload | Implemented |
| Session cache | Implemented |
| CRL support | Implemented |
| Protocol sniffing (TLS vs H1 vs H2) | Implemented |
| JA3 fingerprinting | Implemented |

## Compression

| Algorithm | Status |
|-----------|--------|
| gzip | Implemented |
| Brotli | Implemented |
| zstd | Implemented |
| Deflate | Implemented |

Content negotiation via Accept-Encoding with quality values, MIME-type filtering,
minimum body size threshold, streaming compression.
