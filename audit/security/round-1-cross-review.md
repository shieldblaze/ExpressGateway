# SEC — Round 1 Cross-Review

Owner: `sec`. Round 1 / DISCOVERY.

No `SendMessage` tool is wired in this harness; teammates publish their
inventories under `audit/<area>/round-1-inventory.md`. This document
records points of **agreement** and **disagreement** between SEC's
inventory and the teammates' inventories that exist at the time of
writing. Items that need teammate confirmation are tagged `[NEEDS REPLY]`.

Inventories observed at round close:

* `audit/ebpf/round-1-inventory.md` — present, 471 lines.
* `audit/ebpf/round-1-cross-review.md` — present, 132 lines.
* `audit/reliability/round-1-inventory.md` — present, ~28k bytes.
* `audit/code/round-1-inventory.md` — **not present** at SEC close.
* `audit/protocol/round-1-inventory.md` — **not present** at SEC close.

Round-2 will rerun this exercise against the missing inventories once
they land.

---

## A. SEC ↔ ebpf

### A.1 Agreements

* **CONNTRACK / CONNTRACK_V6 are non-LRU.** Both inventories flag the same
  fact and reach the same conclusion: adversarial flood fills the map
  and starves new flows.
  * SEC finding **S-2** ↔ ebpf §2 "Maps" + ebpf cross-review to-rel #4.
* **`unsafe impl Pod` padding-zero hazard.** SEC **S-9** ↔ ebpf
  cross-review to-`code` #3. Same root cause, same fix shape (force
  zero-init of padding bytes at insert time, or via constructor).
* **No `BPF_LINK_PIN` / map pinning.** Both inventories note process
  restart drops state. SEC framed this as a recovery posture issue;
  ebpf framed it as an operational issue. Same fact.
* **eBPF `unsafe` blocks are bounds-checked.** SEC's unsafe inventory
  (§3.1) and ebpf's "verifier-bypass surface" (cross-review to-`sec`
  ¶4) agree: every `ptr_at` / `read_unaligned` site is preceded by a
  verifier-visible compare against `data_end`. SEC adds a defensive
  caveat about `usize` add overflow that is **not** a real concern on
  BPF64 — withdrawing that to "info" for round 2.

### A.2 Disagreements / additions

* **License section missing in BPF ELF (ebpf §2).** SEC did not look at
  the BPF object's `.license` section in round 1 — this is a real
  startup-only failure mode if aya 0.13's default license string is not
  `"GPL"`. SEC adopts ebpf's concern as a new SEV-1-startup risk for
  round 2. **[NEEDS REPLY]**: ebpf will dig into aya 0.13's
  `LoadOptions::license` default; SEC will mirror that finding once
  resolved.
* **CAP_SYS_ADMIN fallback (ebpf cross-review to-sec ¶2).** SEC implicitly
  assumed the 5.15 floor. The probe at `crates/lb/src/xdp.rs:39-55` only
  checks CAP_BPF + CAP_NET_ADMIN. On pre-5.8 hosts this fails opaquely.
  SEC will add a low-severity round-2 finding: probe should be
  `CAP_BPF || CAP_SYS_ADMIN` (and a similar OR for CAP_NET_ADMIN).
* **Scope boundary on BPF object hygiene.** ebpf wants SEC to own the
  "real-NIC attach security posture" of pinned maps (bpffs mode, owner).
  SEC accepts; round-2 finding will recommend `0o750` on
  `/sys/fs/bpf/expressgateway/` and document the LB-uid model when
  pinning lands in Pillar 4b-3.

### A.3 Scope confirmed shared

* CONNTRACK eviction policy — joint sec + ebpf + rel.
* BPF program license & loader options — primarily ebpf, sec consulted.
* Padding-zero invariant — primarily code, sec + ebpf consulted.

---

## B. SEC ↔ rel

### B.1 Agreements

* **No per-IP / per-listener connection cap (SEC S-4).** rel F-17:
  *"Unbounded. Every accepted connection spawns; no semaphore, no
  max-connections gate at the listener level except QUIC's
  `max_connections: 100_000`."* Same root cause, same listener-loop
  site (`main.rs:1099-1126`). SEC and rel will co-own the round-2
  finding.
* **Slowloris / slow-POST not wired (SEC S-3).** rel F-05 covers the
  TLS-handshake-slowloris variant on top: the rustls accept future is
  not wrapped in any timeout. SEC missed that variant in round 1.
  Adopting rel's F-05 as a sibling finding for round 2.
* **Admin HTTP has no authn / no readiness flip (SEC S-6).** rel §3:
  *"There is no distinction between liveness, readiness, and startup."*
  rel proposes `/livez` + `/readyz` + flip-on-drain. SEC concurs and
  adds: regardless of authn, the readiness endpoint must not leak
  internal state (e.g., backend list, listener counts) without
  authn — `/readyz` should be a binary yes/no.
* **TLS cert rotation is restart-only (SEC §5.1 / rel H2, F-06).**
  rel calls out the doc lies; SEC scoped only the load-site permission
  posture. Same code site (`main.rs:214,611`). Joint round-2 finding
  recommended.
* **No graceful H2 GOAWAY on drain (rel §4.4).** SEC did not look at
  H2 GOAWAY on shutdown in round 1. Adopting rel's observation as a
  protocol-correctness contribution to track for `proto`.

### B.2 Disagreements / additions

* **TLS-listener 0-RTT replay (rel F-19).** rel claims "TCP/TLS
  listeners do not install a `ZeroRttReplayGuard`". SEC notes this is
  expected: TLS 1.3 0-RTT over TCP is opt-in at the rustls server-config
  level (`max_early_data_size`), and the current `build_server_config`
  in `lb-security::ticket::build_server_config` does **not** enable
  early data. So 0-RTT is not active on the TCP path, and no replay
  guard is needed. SEC will check that `build_server_config` truly
  leaves `max_early_data_size = 0` and document the cross-check. If
  rel is right that it's enabled, this becomes a SEV-1.
* **`H2SecurityThresholds` not bound to a per-listener total (rel F-25).**
  rel notes the listener-level in-flight bytes are not bounded. SEC's
  inventory framed this as a DoS concern under "no per-IP cap"; rel
  framed it as a resource-exhaustion concern. Same vulnerability,
  same fix shape (total-inflight-bytes counter + 503). Joint round-2.
* **F-22 — no panic hook.** rel highlights that there is no
  `std::panic::set_hook` and tokio spawned tasks fail silently. SEC
  notes this is more a reliability concern than a security one, but
  there's a security overlap: a panic in the proxy hot path could
  re-encode body bytes via the log line, leaking secrets. SEC will
  ask `code` to confirm that `panic = "abort"` is set in `[profile.release]`
  (`Cargo.toml` shows release profile but no `panic` key, so it
  defaults to `unwind` — see also code teammate's inventory once
  posted).

### B.3 Scope confirmed shared

* Per-listener cap / 503 shed — joint sec + rel + code.
* SIGTERM drain / GOAWAY — proto + rel; sec consulted.
* Health endpoint shape — rel + sec; consult code on rust impl.

---

## C. Blocked — `code` and `proto` inventories not present

The following SEC concerns block on those teammates:

* **Smuggle detector wiring (SEC S-1)** — `proto` is the natural owner
  of "does hyper's H1/H2 enforcement on its own cover the smuggle
  cases the `SmuggleDetector` was designed to handle?" SEC will not
  finalize this finding's severity until `proto` weighs in.
* **`Pod` struct padding (SEC S-9)** — `code` will verify that
  every userspace insert into the BPF maps zeros the `pad` fields
  before insert.
* **io-uring SQE buffer ownership (SEC §3.3 / `crates/lb-io/src/ring.rs:99-182`).**
  SEC flagged the READ/WRITE SQE submissions as needing deeper review
  for buffer lifetime correctness. This is squarely `code`'s domain.
* **`HpackDecoder` reachability.** SEC concluded hyper owns the wire
  and the `lb-h2::HpackDecoder` is internal. `proto` should confirm
  whether the lb-h2 codec is intended to take over from hyper in any
  future pillar.
* **Compression decompress path.** SEC could not enumerate which
  proxy paths invoke `Decompressor` for **request bodies**. `proto`
  to map this on the H1/H2/H3/gRPC matrices.

Once those two inventories land, SEC will append §D and §E to this
cross-review file and revise S-1 / S-9 severities accordingly.

---

## D. Items SEC takes ownership of (round 2)

* **S-1** Smuggle-detector wiring on H1/H2 hot path.
* **S-2** XDP CONNTRACK LRU (joint with `ebpf`).
* **S-3** Slowloris / slow-POST per-listener defense (joint with `rel`).
* **S-4** Per-IP / per-listener concurrent connection cap (joint with `rel`).
* **S-5** 0-RTT replay ring-buffer collapse under unique-token spray.
* **S-6** Admin HTTP authn + readiness flip (joint with `rel`).
* **S-7** Supply-chain tooling in CI (cargo-audit + cargo-deny + verify ignore list).
* **S-8** TLS key file mode-assert at load.
* **S-9** `unsafe impl Pod` padding-zero invariant (joint with `code` + `ebpf`).
* **S-10 (NEW from cross-review with rel)** TLS handshake slowloris on
  `acceptor.accept().await` (`main.rs:1136,1154`).
* **S-11 (NEW from cross-review with ebpf)** CAP_BPF / CAP_SYS_ADMIN
  probe should fallback gracefully on pre-5.8 hosts.
* **S-12 (NEW from cross-review with ebpf)** BPF ELF license section /
  loader license string verification (consult `ebpf`).

---

— `sec`, Round 1.
