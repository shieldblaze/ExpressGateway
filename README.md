# ExpressGateway

A globally distributed load balancer written in Rust, featuring:

- **L7 data plane**: HTTP/1.1, HTTP/2, HTTP/3 with frame-by-frame processing
- **L4 data plane**: XDP/eBPF-based TCP and UDP load balancing
- **QUIC-native proxying** via quiche
- **Native gRPC** proxy with full streaming support
- **Compression**: zstd, brotli, gzip, deflate with streaming and transcoding
- **11 load-balancing algorithms**: round-robin, weighted, P2C, Maglev, ring hash, EWMA, and more
- **Active and passive health checking**
- **Full DoS mitigation catalog**
- **File-backed control plane** with trait seam for future distributed backends
- **Standalone and HA modes**
- **Zero-downtime reload** via `SO_REUSEPORT` and FD passing
- **Panic-free**: zero `unwrap`/`expect`/`panic!` in non-test code

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test --all --all-features
```

## License

GPL-3.0-only
