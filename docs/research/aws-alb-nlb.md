# AWS Elastic Load Balancing (ALB, NLB, GLB)

## Why we study it
AWS ELB is the most-deployed managed load balancer in the world; its public documentation, idle-timeout quirks, and cross-zone behaviour form a baseline that every self-hosted LB is implicitly compared to. For ExpressGateway we care less about AWS's internal implementation (it is not public) and more about its externally visible contracts — what clients expect, what operators assume, and where managed-LB assumptions break down.

## Architecture in brief
AWS offers three flavours:

**Application Load Balancer (ALB).** L7, HTTP/HTTPS/HTTP-2/gRPC. Listeners accept TLS, de-mux by host/path to target groups, forward to targets (EC2 instances, ECS tasks, Lambda, IPs). Target health is HTTP-probed. ALB is a managed, multi-AZ fleet behind a DNS name; AWS does not disclose the data-plane implementation but published behavior (sticky-session cookies, XFF semantics, idle timeout) implies a conventional L7 reverse proxy.

**Network Load Balancer (NLB).** L4, TCP/UDP/TLS, flow-hash based selection, preserves client IP by default. NLB is built on AWS Hyperplane, a shared scale-out NAT/LB substrate for intra-AWS traffic. It supports millions of connections per second, sub-ms jitter, and is the closest managed analogue to Katran/Unimog.

**Gateway Load Balancer (GLB).** L3, GENEVE-encapsulated transparent forwarder; lets operators insert appliances (firewalls, IDS) in the path while preserving the original packet. Uses the same Hyperplane-style flow-hash.

All three are DNS-fronted; clients resolve `my-alb.elb.amazonaws.com` to a set of A records that change over time as AWS scales the LB nodes. Cross-zone load balancing is a policy toggle: on, requests distribute across AZs; off, they stay local (with failover).

Control plane is AWS's internal orchestration; operators interact via the ELB API (RegisterTargets, ModifyListener, etc.). Target health state is eventually consistent.

Concurrency/storage: opaque. The public behaviour implies NLB's connection-tracking is kept in a distributed store (Hyperplane) and ALB retains session stickiness via signed cookies.

Language: internal, not disclosed. Hyperplane is believed to be C++/Rust; ALB data plane is likely a heavily modified NGINX or in-house equivalent.

## Key design patterns
1. **DNS-fronted LB endpoint with changing A records.** Clients that cache DNS aggressively get stuck on dying LB nodes — AWS caps TTLs at 60 seconds and publishes "don't cache longer" guidance.
2. **Signed sticky cookies for session affinity (ALB).** Stateless stickiness: the cookie contains the target-id signed with an LB secret. Survives LB scale events.
3. **Flow-hash per 5-tuple (NLB).** TCP flows pinned by hash to a target; preserves source IP so the target sees the client.
4. **Cross-zone toggle with documented cost model.** Cross-zone equalises load but incurs inter-AZ bandwidth; operators see the trade-off explicitly.
5. **Target groups as the unit of targeting.** Listeners forward to target groups, target groups attach to instances — a two-level indirection that lets the same backends serve multiple listeners.
6. **Connection draining / deregistration delay.** When a target is deregistered, existing connections are allowed to finish up to a configured window (default 300s).
7. **Health checks with separate healthy/unhealthy thresholds.** Prevents flap; configurable per target group.
8. **GENEVE encapsulation for transparent appliances (GLB).** Preserves original packet, lets the appliance see L3-through-L7 unchanged.
9. **ALB access logs to S3.** Structured logs batched to S3; asynchronous, bounded-staleness (up to 5 minutes).
10. **Direct TLS termination with AWS Certificate Manager integration.** Certs rotate transparently; operators never hold private keys.

## Edge cases / hard-won lessons
- **NLB idle timeout is 350 seconds and not configurable.** Long-idle TCP connections without keepalive get RST-without-notice. This is the single most-cited NLB gotcha.
- **ALB idle timeout default is 60 seconds.** WebSocket applications that don't send heartbeats break silently; operators bump to the maximum 4000 s.
- **Client IP preservation only when connection is source-aware.** NLB preserves by default; ALB uses `X-Forwarded-For` (which can be spoofed if the listener doesn't strip it).
- **`Connection: keep-alive` between ALB and target is separately configured.** Default keep-alive is long; application idle closes can cause ALB to get a RST on pool reuse.
- **Sticky-session cookie domain.** ALB-generated cookies are set at the ALB domain; application cookies don't compose cleanly without custom stickiness.
- **Cross-zone off + AZ failure drains traffic unevenly.** AWS returns A records for the surviving AZs, but DNS TTLs mean clients fail slow.
- **Scaling events change LB-node IP addresses.** Clients that pinned an IP (not the DNS name) see connection refused as AWS scales up/down.
- **GLB appliance flow symmetry.** Return path must hash to the same appliance; operators who forget symmetric routing see asymmetric drops.
- **HTTP/2 to HTTP/1 downgrade on ALB** drops some headers (e.g. `te: trailers`); gRPC over HTTP/1 is unsupported — gRPC requires HTTP/2 end-to-end or a dedicated gRPC target group.
- **Slow-POST vulnerability historically** — ALB did not enforce body-read deadlines tightly; mitigated in later updates but still a documented concern.
- **Hyperplane cross-AZ failover is not instant.** A zone-failure event can cause seconds of packet loss before NLB converges.
- **Access-log batch lag hides incident timelines.** 5-minute publish delay means near-real-time incident response relies on CloudWatch metrics, not logs.

## Mapping to ExpressGateway
| AWS ELB pattern | Our equivalent | Gap / status |
| --- | --- | --- |
| L4 flow-hash / Maglev | `crates/lb-l4-xdp/` + `crates/lb-balancer/src/maglev.rs` | Present. |
| L7 listener with target groups | `crates/lb-core/src/cluster.rs` + `crates/lb-l7/` | Cluster abstraction present; multi-listener routing to shared clusters is implicit via config. |
| Signed sticky cookie | `crates/lb-balancer/src/session_affinity.rs` | Session affinity present; signed-cookie issuance is out of scope for v1. |
| Connection draining on deregistration | `crates/lb-health/src/lib.rs` + `crates/lb-controlplane/src/lib.rs` | Health-based removal present; explicit "deregistration delay" knob is a gap. |
| Cross-zone toggle | (none) | Single-node scope; irrelevant for now. |
| Health checks with thresholds | `crates/lb-health/src/lib.rs` | Present. |
| Idle timeout (configurable) | Per-protocol crates | Configurable per protocol; a documented default must be chosen carefully (ALB lesson: too short breaks WebSockets; NLB lesson: too long leaks sockets). |
| Access log to durable store | `crates/lb-observability/src/lib.rs` | Metrics/traces present; S3-style batched logs are out of scope. |
| gRPC end-to-end H2 | `crates/lb-grpc/`, `tests/grpc_*.rs` | Present; no H1 downgrade path for gRPC. |
| TLS termination with auto-rotate | `crates/lb-config/src/lib.rs` + reload in `crates/lb-controlplane` | Reload plumbing present; cert-rotation-specific flow is a policy we should formalise. |

## Adoption recommendations
- We should publish explicit default idle timeouts per protocol in `lb-h1`, `lb-h2`, `lb-h3`, and `lb-quic` with rationale — longer than ALB's 60s but shorter than NLB's 350s, with a documented "WebSocket / CONNECT tunnel" override.
- We could add a deregistration-delay knob to `lb-controlplane`: when the config reload removes a backend, hold it in "draining" state for N seconds before actually evicting in-flight connections. Mirrors ALB's connection-draining contract.
- We should keep `crates/lb-grpc` end-to-end H2; do not attempt H2→H1 downgrade for gRPC traffic. ALB's lesson is that it's user-hostile.
- We could formalise the signed-sticky-cookie pattern as an optional `session_affinity` mode; useful for clients that cannot use source-IP affinity (mobile clients behind carrier NAT).
- We should surface TCP keepalive configuration in `lb-io` — both ALB and NLB guidance emphasises that applications must send heartbeats to survive the proxy's idle timeout.
- We should document that our DNS-level concerns (A-record TTL, client caching) are out of our direct control and leave that to the deployment layer.
- We should keep our observability path close to real-time (metrics in sub-second), treating the AWS batch-log lag as a cautionary example rather than a model.

## Sources
- https://docs.aws.amazon.com/elasticloadbalancing/latest/application/ — ALB user guide.
- https://docs.aws.amazon.com/elasticloadbalancing/latest/network/ — NLB user guide; idle-timeout section.
- https://docs.aws.amazon.com/elasticloadbalancing/latest/gateway/ — GLB user guide; GENEVE.
- https://aws.amazon.com/blogs/networking-and-content-delivery/ — ongoing posts on ELB internals and best practices.
- "A Decade of Hyperplane" and related AWS re:Invent talks (NET305, NET307, NET401 class sessions, indexed on YouTube).
- https://aws.amazon.com/blogs/networking-and-content-delivery/application-load-balancer-idle-timeout/ — idle-timeout discussion.
- https://docs.aws.amazon.com/elasticloadbalancing/latest/application/sticky-sessions.html — sticky-cookie scheme.
- https://docs.aws.amazon.com/elasticloadbalancing/latest/application/load-balancer-access-logs.html — access-log format and batching.
