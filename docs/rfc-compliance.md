# RFC Compliance Report

## HTTP/1.1 -- RFC 7230-7235 / RFC 9110-9112

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Request-line parsing | Compliant | `hyper` crate handles parsing |
| Header field parsing | Compliant | Case-preserved for H1 |
| Chunked transfer encoding | Compliant | Automatic H1 ↔ H2/H3 translation |
| Content-Length handling | Compliant | Preserved when known |
| Connection keep-alive | Compliant | `protocol-http/src/h1/` |
| Pipelining | Compliant | `protocol-http/src/h1/pipelining.rs` |
| Hop-by-hop header removal | Compliant | `headers.rs:strip_hop_by_hop()` |
| Via header | Compliant | Inserted on forwarding |
| Host header validation | Compliant | Required for HTTP/1.1 |
| Transfer-Encoding stripping | Compliant | Removed at proxy boundary |
| URI normalization | Compliant | Double-encoded dot detection (zero-alloc) |
| Max header size | Enforced | Configurable (default 8KB) |
| Max URI length | Enforced | Configurable (default 4KB) |
| Request body size limits | Enforced | `body_limits.rs` |

## HTTP/2 -- RFC 7540 / RFC 9113

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Stream multiplexing | Compliant | Preserved across proxy |
| Flow control (stream-level) | Compliant | `h2/flow_control.rs`, overflow-protected |
| Flow control (connection-level) | Compliant | `connection_window_update()` with max check |
| GOAWAY handling | Compliant | Graceful and immediate modes |
| RST_STREAM handling | Compliant | Per-stream error signaling |
| Pseudo-headers (:method, :path, :scheme, :authority) | Compliant | Proper H1 ↔ H2 translation |
| Header compression (HPACK) | Compliant | Via `h2` crate |
| TE header filtering (Section 8.2.2) | **Fixed** | Only `trailers` allowed; was incorrectly stripping all TE |
| H2C upgrade | Compliant | `h2/h2c.rs` |
| CONNECT method | Compliant | `h2/connect.rs` |
| Window max 2^31-1 (Section 6.9.1) | **Fixed** | `MAX_FLOW_CONTROL_WINDOW` constant, CAS overflow check |
| Stream count limits | Compliant | `max_concurrent_streams` enforced |

### Fixes Applied
1. **TE header (Section 8.2.2)**: `strip_hop_by_hop()` was removing `TE: trailers` which is explicitly allowed and required for gRPC. Fixed to preserve `trailers` value only.
2. **Flow control overflow (Section 6.9.1)**: `FlowWindow::replenish()` had no upper bound. Added `MAX_FLOW_CONTROL_WINDOW = (1 << 31) - 1` with CAS-based overflow protection.

## HTTP/3 -- RFC 9114

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| QUIC transport | Compliant | Via `quinn` crate |
| Request/response framing | Compliant | Via `h3` crate |
| QPACK header compression | Compliant | Via `h3` crate |
| Stream multiplexing | Compliant | Preserved across proxy |
| Flow control | Compliant | QUIC-level flow control |
| CONNECTION_CLOSE handling | Compliant | Proper error propagation |
| 0-RTT | Supported | Where safe (idempotent requests) |
| Connection migration | Supported | Via quinn |

### Fix Applied
1. **Connection header stripping**: `is_connection_header()` was incorrectly treating `Proxy-Authenticate` and `Proxy-Authorization` as connection-specific. These are end-to-end authentication headers per RFC 9110 Section 11.7.

## QUIC -- RFC 9000, 9001, 9002

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Connection establishment | Compliant | Via `quinn` |
| Stream management | Compliant | Via `quinn` |
| Flow control | Compliant | Via `quinn` |
| Loss detection (RFC 9002) | Compliant | Via `quinn` |
| Congestion control (RFC 9002) | Compliant | Via `quinn` |
| TLS 1.3 integration (RFC 9001) | Compliant | Via `quinn` + `rustls` |
| Connection ID | Compliant | Used for XDP steering |

## WebSocket -- RFC 6455

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Opening handshake | Compliant | `upgrade.rs` with Sec-WebSocket-Accept |
| Frame types (text, binary, ping, pong, close) | Compliant | `frames.rs` |
| Close handshake | Compliant | Close frame + response |
| Masking (client → server) | Compliant | Via `tungstenite` crate |
| Fragmentation | Compliant | Via `tungstenite` crate |
| Close codes (Section 7.4.1) | Compliant | `error.rs:close_code()` mapping |
| Subprotocol negotiation | Compliant | `upgrade.rs:extract_subprotocols()` |

## WebSocket over HTTP/2 -- RFC 8441

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Extended CONNECT method | Compliant | `h2_websocket.rs` |
| `:protocol` pseudo-header | Compliant | `H2Protocol` type |
| Bidirectional streaming | Compliant | Via H2 stream |

## TCP -- RFC 9293

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Connection state machine | Compliant | Tokio TCP handling |
| Half-close (FIN handling) | Compliant | `into_split()` with independent shutdown |
| Keep-alive | Compliant | Configurable idle/interval/count |
| Backlog management | Compliant | `SO_BACKLOG` configurable (default 50000) |

## UDP -- RFC 768

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| Datagram delivery | Compliant | Per-session backend sockets |
| No connection state | Compliant | Session tracking is application-level |

## PROXY Protocol -- v1 (text) / v2 (binary)

| Requirement | Status | Implementation |
|-------------|--------|----------------|
| v1 text format | Compliant | Iterator-based zero-alloc parsing |
| v2 binary format | Compliant | Magic byte detection, TLV support |
| Address families | Compliant | TCP4/6, UDP4/6, UNIX stream/dgram |
| TLV extensions | Compliant | ALPN, SSL, Authority, CRC32C, NOOP, UniqueID, Netns |
| Auto-detection | Compliant | v1 vs v2 magic byte check |
