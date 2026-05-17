### ROUND8-OPS-06 — Tracing context propagation library exists but ZERO L7 callsites wire it

Reference: `audit/round-8/research/pingora.md` architecture summary ("Multithreaded Tokio, work-stealing … the connection pool is shared") — propagation across worker threads is the operator's job; Envoy threading model. REL-2-07 in `audit/reliability/round-2-review.md:342` documents the same gap, status: `Verified-Fixed-Partial(verifier=proto, author-sha=1d462c7) — W3C traceparent/tracestate codec ships in lb_observability::tracing_propagation … NO L7 callsite extracts or injects today`.
Our equivalent: `crates/lb-observability/src/tracing_propagation.rs` (full W3C codec, span-name vocabulary, OtlpConfig). `grep "extract_parent\|inject_context\|parse_traceparent\|tracing_propagation::" crates/lb-l7 crates/lb-quic` → zero hits. `grep "info_span\|tracing::span" crates/lb-l7 crates/lb-quic` → zero hits.

Severity: high
Status:   Verified-Fixed(verifier=verify, 6253ad9a)   <!-- VERIFIED: grep -rn tracing_propagation crates/ --include=*.rs | grep -v test non-empty: trace_ctx.rs:190 extract_parent + :240 inject_into + h1_proxy.rs:1382-1386 header injection. No longer zero-callsite (REL-2-07 recheck). See audit/round-8/verify/ops.md. --><!-- prior div-l7 note: FIRST production L7 callsite of lb_observability::tracing_propagation: new crates/lb-l7/src/trace_ctx.rs opens a per-request span (lb.l7.request: trace_id/span_id/parent_id/http.method/http.target/http.status_code/net.sni/listener) on the H1 + H2 entry and injects a refreshed child traceparent onto the upstream request, incl. the ROUND8-L7-01 WS-upgrade dial. Span propagated via Instrument (never an Entered guard across .await). Proof: round8_traceparent_propagation.rs 1/1 (span carries inbound trace_id + records http.status_code) + round8_ws_upgrade_defer::upstream_receives_child_traceparent + 4 trace_ctx unit tests. Verification teammate to confirm + close. -->
Status (pre-fix):   Open

Divergence:
- Production references all rely on per-request span propagation: a request-scoped `traceparent` header is forwarded to the upstream, and the LB emits a child span for the proxy work. Without this, "5% of requests slow — where is the latency?" is unanswerable.
- We have the *library* but no consumer. The L7 hot path emits connection-level `tracing::info!` lines only.
- The REL-2-07 status note already discloses this as "library-only fix; proxy wire-in is deferred." Round-7 final report graded this CONDITIONAL GO. Round-8 stance is to NOT trust the prior verdict — this is still a divergence and we should re-tag it.

Impact:
- Distributed traces from any upstream service that emits `traceparent` (Service A → ExpressGateway → Service B) show a discontinuity at our hop. The trace either ends at A (we strip the header) or skips to B with no LB span (we forward the header unmodified, but no child span is emitted).
- On-call debugging "where did the 500 ms go" requires log correlation by timestamp + 5-tuple instead of trace pivot. This is exactly the gap REL-2-07 originally identified.

Reproduction:
1. Start ExpressGateway with `[observability].otlp_endpoint = "http://collector:4317"`.
2. Send: `curl -H 'traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01' https://gw/path`.
3. Inspect collector — no span with the trace id `0af7651916cd43dd8448eb211c80319c` from `expressgateway`. The library at `crates/lb-observability/src/tracing_propagation.rs:25-33` defines the header constants and `parse_traceparent` but no L7 entry calls them.

Recommendation:
1. In `crates/lb-l7/src/h1_proxy.rs::handle` (and the H2/H3/gRPC equivalents), at the top of each request, call `lb_observability::tracing_propagation::extract_parent(request.headers())` and open `tracing::info_span!("lb.l7.h1.request", trace_id = %ctx.trace_id_hex(), parent_id = %ctx.parent_id_hex(), http.method = %method, http.target = %uri, listener = %listener)` inside an `Instrument` wrapper.
2. Before calling the upstream client (hyper / quiche / h3), call `lb_observability::tracing_propagation::inject_into(request.headers_mut(), &child_ctx)` so the upstream sees a fresh `traceparent` with our span as parent.
3. On status capture (response builder exit), record `http.status_code` + `http.response_content_length` on the span.
4. Wire OTLP exporter from `[observability].otlp_endpoint` (already in OtlpConfig). Default off; document.
5. Update REL-2-07's status to remain `Verified-Fixed-Partial` (it already is) but add a Round-8 reconcile note that the wire-in has not happened in subsequent rounds, escalating the severity.

Notes:
- This finding is a "missed defense" not because the reference identified a CVE in our code, but because production references (Envoy, Pingora, HAProxy) all consider distributed tracing a baseline operability feature for an edge LB. REL-2-07 acknowledged it; subsequent rounds did not close the loop.
