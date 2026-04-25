# Benchmarks

## Microbenchmarks (Criterion)

Criterion benchmarks live in each crate's `benches/` directory:

- `crates/lb/benches/balancer_bench.rs` — LB algorithm selection latency
  (round-robin, weighted, least-conn, Maglev, P2C, consistent hash, EWMA)

Run all benchmarks:

```bash
cargo bench --workspace
```

Run a specific benchmark group:

```bash
cargo bench -p expressgateway-lb -- l4_round_robin
```

## Load Tests (wrk2 + h2load)

### HTTP/1.1 — wrk2

```bash
# Start ExpressGateway
cargo run --release

# Constant-throughput test (Coordinated Omission corrected)
wrk2 -t4 -c100 -d30s -R10000 --latency http://localhost:8080/
```

### HTTP/2 — h2load

```bash
# Multiplexed streams
h2load -n100000 -c10 -m100 http://localhost:8080/
```

### HTTP/3 — curl

```bash
curl --http3 https://localhost:8443/ -w "@bench/curl-format.txt"
```

## Head-to-Head Comparison

To compare against nginx and Envoy on identical hardware:

1. Configure all three with equivalent routes (single upstream, no TLS).
2. Use wrk2 with `-R` flag for constant throughput.
3. Capture HdrHistogram output for p50/p99/p999/p9999.
4. Record results in `docs/benchmark_results.md`.

## Allocation Profiling

```bash
# dhat (requires nightly)
cargo +nightly run --release --features dhat-heap

# heaptrack
heaptrack ./target/release/expressgateway
heaptrack_gui heaptrack.expressgateway.*.zst

# perf + flamegraph
perf record -g ./target/release/expressgateway &
wrk2 -t4 -c100 -d10s -R5000 http://localhost:8080/
perf script | inferno-collapse-perf | inferno-flamegraph > flamegraph.svg
```
