# S47 — Load-balancing algorithm + health-ejection correctness review

Reviewer: balancer/health correctness (Rust). Branch `review/s47-rfc-security`
(from `main` @ `01915a77`). **Read-and-reason only — no cargo command was run on
this box** (2 vCPU / 7 GB / 11 GB free); every claim below is derived from the
source and is marked with what would prove or disprove it in CI.

Scope reviewed line-by-line: `crates/lb-balancer/src/*` (13 files),
`crates/lb-health/src/{ejection.rs,lib.rs}`, `crates/lb-core/src/{backend.rs,
cluster.rs,policy.rs,authority.rs}`, plus every call site that reaches them
(`crates/lb-l7/src/upstream.rs`, `crates/lb-l7/src/{h1,h2}_proxy.rs`,
`crates/lb/src/main.rs`, `crates/lb-quic/src/{passthrough.rs,conn_actor.rs}`,
`crates/lb-l4-xdp/src/lib.rs`) and the test files that touch them (the 12 `tests/balancer_*.rs`,
`tests/l4_xdp_maglev.rs`, `crates/lb-balancer/tests/*` (2),
`crates/lb-health/tests/ejection_controls.rs`,
`crates/lb-l7/tests/health_ejection_{e2e,byte_identical}.rs`) plus every in-crate
`#[cfg(test)]` module in scope.

## Prior art honoured (NOT re-reported)

Read first: `docs/known-limitations.md`, `docs/features.md`, `audit/deferred.md`,
`audit/code/`, `SECURITY.md`. The following are documented and are treated as
constraints, not defects:

* Only round-robin (L7 + raw-TCP) and Maglev-by-Connection-ID (QUIC Mode A) are
  live; the other ten algorithms are library-only with no policy key.
* Per-backend `weight` is accepted but not enforced.
* EWMA is "unfed"; Maglev-for-L4 (ROUND8-L4-04) is deferred.
* H3-front `select_backend` returns `backends[0]` and neither feeds nor filters
  (`conn_actor.rs:1210` — the comment matches `known-limitations.md` exactly).
* gRPC is filtered-but-not-fed; `Http2PoolError::Send` is deliberately discarded;
  the L4 leg feeds dial outcomes only. **All three verified against the code and
  the doc table is accurate.**

Findings below are new, or are a documented item whose *magnitude* is
mis-stated in the docs.

---

## BAL-01 — the minimum-healthy floor is double-spent by half-open re-ejection; a 2-backend listener reaches ZERO admitted backends

Severity: **HIGH** · **LIVE-PATH** (ejection is live on H1/H2/WS/L4 per
`features.md`) · Blocking for prod: **YES** (documented safety property is false)

File: `crates/lb-health/src/ejection.rs:297-305`, with
`ejection.rs:289-291` and `ejection.rs:450-455`.

```rust
// ejection.rs:297-305 — the half-open failure arm
match entry.ejected_until {
    // The half-open probe failed: this entry was already paid for against the floor at
    // its first ejection, so back off further without re-checking it.
    Some(deadline) if now >= deadline => {
        entry.rounds = entry.rounds.saturating_add(1);
        let window = backoff(policy, entry.rounds);
        entry.ejected_until = Some(now + window);
        FailureAction::ReEjected(window)
    }
```

```rust
// ejection.rs:450-455 — a half-open backend is NOT counted as ejected
fn count_ejected(entries: &HashMap<SocketAddr, Entry>, now: Instant) -> usize {
    entries
        .values()
        .filter(|e| e.ejected_until.is_some_and(|deadline| now < deadline))
        .count()
}
```

The comment's premise — "already paid for against the floor at its first
ejection" — is false. While a backend sits half-open (deadline passed, no
outcome recorded yet) `count_ejected` **excludes** it, which *refunds* its floor
slot to the pool. Another backend may then spend that slot permanently. When the
half-open backend's own failure lands it re-claims a slot with **no
`can_eject` check at all** — neither the percentage floor nor the absolute
"never leave zero admitted" floor. The budget is spent twice.

The refund window is not narrow: it is one full upstream failure latency (a dial
or handshake timeout — seconds, by construction of `UpstreamErrorClass::Timeout`),
during which every peer failure recorded is eligible to take the freed slot.

**Concrete failure scenario** — default policy (`consecutive_failures = 5`,
`base_ejection = 30 s`, `min_healthy_percent = 50`), listener with backends
`A`, `B` (`can_eject`: `max_ejectable = 2*50/100 = 1`), correlated outage (shared
DB down) that outlasts 30 s:

| t | event | state |
|---|---|---|
| 0 s | `A` fails 5× | `A` ejected until 30 s; `count_ejected = 1` |
| 1 s | `B` fails 5× | `can_eject(2, 1)` → `after=2 > 1` → **Suppressed**, `B` stays. Floor works. |
| 30 s | `A`'s deadline passes | `admits(A) = true`; `count_ejected = 0` (refund) |
| 30 s+ | `B` fails again (still `Unhealthy`, `ejected_until = None`) → falls to the plain `None =>` arm → `can_eject(2, **0**)` → true | **`B` ejected until 60 s** |
| 30 s+ε | `A`'s half-open probe fails → `Some(deadline) if now >= deadline` arm, **no floor check** | **`A` re-ejected until 90 s** |

Result: `ejected_count() == 2` of 2. `admits(A) == false`, `admits(B) == false`
— **zero admitted backends**, the exact state the module header
(`ejection.rs:4-6`) says is impossible ("a correlated outage ejects the whole
fleet and the listener serves nothing") and that `features.md` sells as a
guarantee ("at most `100 - min_healthy_percent` of a listener's backends may be
ejected … plus an absolute *never eject the last backend* rule").

Generalised: the ejected set grows by one every time a half-open window elapses
while peers are failing. **N=4 converges to 3 ejected / 1 admitted (25 % healthy,
not the documented 50 %).**

**Impact.** Availability is contained by fail-open in both pickers
(`upstream.rs:192`, `main.rs:3065`), so traffic still flows — ejection silently
degrades to *no ejection*. The real cost is on the recovery edge: after a
correlated *transient* outage (e.g. a 60 s partition), 3 of 4 backends stay
ejected with exponentially-grown windows (up to `max_ejection = 300 s`) while the
single admitted backend absorbs **100 % of the restored load instead of 50 %**.
That backend then times out under 4× its share, its failures cannot eject it (the
floor now refuses), and the overload persists for minutes. This is the
"partial outage → total outage" mode `max_ejection_percent` exists to prevent.
Secondary: `backend_ejected` / `ejected_count` metrics report a state the docs
call impossible, so the operator's dashboard is wrong exactly during the incident.

**Second trigger, same root cause (floor evaluated only at ejection time).**
`HealthRegistry::reseed` (`ejection.rs:213-225`) preserves survivors' ejections
across a reload. Pool `[A,B,C,D]` with `A`,`B` legitimately ejected, then a config
reload drops `C`,`D` → `reseed(&[A,B])` retains both ejections → 100 % of the new
pool is ejected with no failure recorded at all.

**Would an existing test catch it?** **No.**
`crates/lb-health/tests/ejection_controls.rs:127`
(`all_backends_failing_does_not_eject_everything`) and
`crates/lb-l7/tests/health_ejection_e2e.rs:274`
(`all_backends_failing_keeps_serving_degraded`) both drive all their failures
*inside the first ejection window* (no sleep before the assert; the e2e drives 24
sequential requests against accept-then-drop backends, far under
`BASE_EJECTION = 600 ms`), so the half-open arm never executes. Both assert
`ejected_count() == 1`. The catching change is one line: sleep past
`base_ejection`, keep failing, re-assert `ejected_count() <= max_ejectable`.
`can_eject_floor_arithmetic` (`ejection.rs:612`) tests the *function* in
isolation and is correct — the bug is that the function is not called on the
re-ejection path.

---

## BAL-02 — `HealthFilteredPicker`'s retry loop assumes a sequential rotation; under concurrent picks it fails open to an ejected backend while a healthy one exists

Severity: **MEDIUM** · **LIVE-PATH** (every H1/H1s/H2 listener; twin at the L4
accept loop) · Blocking for prod: **NO** (degrades ejection, does not break it)

File: `crates/lb-l7/src/upstream.rs:160-192`; twin at `crates/lb/src/main.rs:3035-3055`.

```rust
// upstream.rs:161-163 — the claim
/// Bounded by the backend count: one pass over the rotation is enough to find an admitted
/// backend if one exists, and a picker with a stuck counter cannot spin.
max_attempts: usize,
...
// upstream.rs:183-192
let mut last = None;
for _ in 0..self.max_attempts {
    let backend = self.inner.pick_info()?;
    if self.gate.admits(backend.addr) {
        return Some(backend);
    }
    last = Some(backend);
}
last
```

`RoundRobinUpstreams::pick_info` (`upstream.rs:127-137`) takes the shared
`Mutex<usize>`, reads `*g % len`, increments, releases. Nothing holds the counter
across the retry loop, so the "one pass over the rotation" claim holds **only
single-threaded**. With `T` concurrent picks (the production condition — one
`Arc<H1Proxy>` serving every connection), thread X's k-th attempt sees counter
`c_k` where `c_k − c_{k−1} = 1 + (peer picks in between)`, i.e. an arbitrary
stride mod `N`. The loop can therefore visit the same ejected index `N` times and
fall out to `last` — an **ejected** backend — while an admitted one existed.

**Concrete failure scenario.** `N = 2`, backends `[X (ejected), Y (healthy)]`,
two busy worker threads. Under a lock-step interleaving (T1,T2,T1,T2 …) T1 draws
counters 0,2 → index 0 = `X` both times → exhausts `max_attempts = 2` → fails
open to the ejected `X`. In that steady state **50 % of requests go to the dead
backend** rather than ~0 %. Modelling the strides as arbitrary mod `N` with `h`
admitted of `N`, the spurious-fail-open rate is `(1 − h/N)^N` — 25 % at
`N = 2, h = 1`, ~30 % at `N = 3, h = 1`, → `1/e ≈ 37 %` asymptotically. Each such
request is a 502 that ejection was supposed to prevent.

The L4 twin (`main.rs:3036`, `for _ in 0..state.backends.len().max(1)` around a
`state.balancer.lock()` that is released between iterations) is the identical
defect on `PlainTcp`/`Tls` listeners.

Note the interaction with BAL-01: once *every* backend is ejected the loop always
burns `N` inner picks, which also breaks the R3 "inner picker advanced exactly
once per pick" property that `upstream.rs:150-155` documents.

**Would an existing test catch it?** **No.** All six `HealthFilteredPicker` tests
(`upstream.rs:279-390`) and all five e2e controls
(`crates/lb-l7/tests/health_ejection_e2e.rs`) are strictly sequential — `drive()`
awaits each request before issuing the next. There is no concurrent test of any
picker anywhere in the repo. A catching test: 4 tokio tasks × 200 picks against
`DenyList(vec![a])` with `N=2`, asserting the denied address is never returned.

---

## BAL-03 — Maglev backend ids embed the list index, so any membership change re-randomises the whole table (disruption ≈ (N−1)/N, not 1/N)

Severity: **MEDIUM** · **LIVE-PATH file, currently-dormant property** ·
Blocking for prod: **NO** today; **YES** before Mode A backends become
reloadable or a table-miss falls back to a Maglev pick

File: `crates/lb-quic/src/passthrough.rs:889-896` (production) and
`crates/lb-quic/src/passthrough.rs:1201-1213` (a duplicated copy in the test
helper). Same anti-pattern at `crates/lb/src/main.rs:1606`.

```rust
// passthrough.rs:889-901
let backends: Vec<Backend> = params
    .backends
    .iter()
    .enumerate()
    .map(|(i, sa)| Backend {
        id: format!("backend-{i}-{sa}"),   // <-- position baked into the identity
        ...
let maglev =
    Maglev::new(&backends).map_err(|e| std::io::Error::other(format!("maglev: {e}")))?;
```

`Maglev::permutation` (`maglev.rs:35-43`) derives `(offset, skip)` **from
`b.id`**. Making the id position-dependent means a backend's permutation — its
entire preference sequence over the 65537-slot table — changes when its *index*
changes, even though the backend itself did not.

**Concrete failure scenario.** Pool `[A,B,C,D]`; the operator removes `B`.
Ids before: `backend-0-A, backend-1-B, backend-2-C, backend-3-D`; after:
`backend-0-A, backend-1-C, backend-2-D`. `C` and `D` are, to the table, brand-new
backends: their offsets/skips are unrelated to the old ones, and the fill
interleaving they drive also perturbs `A`'s claims. Removing the **first**
backend renames *all* `N−1` survivors, so the rebuilt table is statistically
independent of the old one — a surviving key keeps its backend with probability
≈ `1/(N−1)` (i.e. chance) instead of ≈ 1. Correct Maglev remaps ~`1/N` of keys on
a removal; this remaps ~all of them. The whole reason Maglev was chosen over
`hash % N` is thereby void.

**Why it is dormant today (stated honestly).** Mode A's table is built once in
`PassthroughListener::spawn` and `[passthrough]` changes are restart-required
(`lb-config/src/reload.rs:107,221`), so the set never changes in-process; and a
short-header table miss **drops** the packet (`passthrough.rs:800`) rather than
falling back to a Maglev pick, so cross-instance table agreement is not currently
load-bearing. Two ordinary next steps make it live and total: (a) making
passthrough backends reloadable, (b) Maglev-routing a flow-table miss — which is
what makes multi-instance ECMP work and is exactly what `features.md:67` promises
("the same client flow keeps landing on the same backend"). The `main.rs:1606`
site (`format!("backend-{i}")`) activates the instant any hash-based policy is
exposed on the L7 path.

**Would an existing test catch it?** **No — and this is a coverage defect in its
own right.** Every disruption test only ever removes or adds at the **tail**,
where no survivor's index changes:
`maglev.rs:180` `test_minimal_disruption` uses `backends_5.iter().take(4)`;
`tests/balancer_maglev.rs:29` compares `make_backends(5)` vs `make_backends(6)`;
`ring_hash.rs:165` and `tests/balancer_ring_hash.rs:26` do the same. A removal
from the *middle* or the *front* — the case that fails — is untested. The bars are
also far too loose to detect partial damage: `> 60 %` (`maglev.rs:205`,
`ring_hash.rs:191`) and `> 50 %` (`tests/balancer_maglev.rs:60`) where the correct
answer is ~100 %.

Fix direction (not applied per the brief): id = the resolved address only, and
sort the list before building (see BAL-04).

---

## BAL-04 — Maglev / ring-hash tables are built from an un-canonicalised (unsorted) backend list, so two instances with the same backend SET but a different order disagree

Severity: **MEDIUM** · **LIBRARY-ONLY** (live for Mode A only in the dormant
sense of BAL-03) · Blocking for prod: **NO**; blocking for any multi-instance
consistent-hash claim

File: `crates/lb-balancer/src/maglev.rs:60-102` (`populate`),
`crates/lb-balancer/src/ring_hash.rs:31-41`, callers
`passthrough.rs:889` and `main.rs:1599-1606`.

```rust
// maglev.rs:70-85 — the fill order IS the list order
while filled < TABLE_SIZE {
    for i in 0..n {
        ...
        let mut slot = (offset + c * skip) % TABLE_SIZE;
        while table.get(slot).copied() != Some(usize::MAX) {
            c += 1;
            slot = (offset + c * skip) % TABLE_SIZE;
        }
```

Maglev's populate is a round-robin over the list; in each round the earlier
entries claim contested slots first. The per-backend permutation is
order-independent, but **which backend wins a contested slot is not**. Google's
implementation canonicalises by sorting backend names for exactly this reason;
neither `Maglev::new` nor `RingHash::new` sorts, and no caller sorts before
calling. Two gateway instances given the same backend *set* in a different order
(different config-generation automation, a hand-edited config, a rolling config
push in flight, or a re-resolved DNS answer reordering the list) produce tables
that differ on every slot that was contested during the fill — the uncontested
prefix agrees, the tail does not, and the tail is where most collisions land.
This is invisible to any single-instance test.

`RingHash` is *not* affected in the same way (its vnode hashes come only from
`id` + vnode number and it sorts the ring at `ring_hash.rs:41`), so only the
`backend_idx` numbering depends on order — which is why removing a *front*
backend would show as "0 % stable" in a ring-hash test that compares indices
rather than ids. That is a test-methodology hazard, not a code bug.

**Would an existing test catch it?** No test builds two tables from the same set
in two orders. One assert would cover it:
`Maglev::new(&[a,b,c])` vs `Maglev::new(&[c,a,b])` must agree on the *id* a key
maps to for ≥ 99 % of keys.

---

## BAL-05 — "EWMA" implements no exponentially-weighted moving average: no `alpha`, no time constant, no decay, no aging

Severity: **MEDIUM** · **LIBRARY-ONLY** · Blocking for prod: **NO**; blocking
for exposing `LbPolicy::Ewma`

File: `crates/lb-balancer/src/ewma.rs:20-58` + `crates/lb-core/src/backend.rs:142-144`.

```rust
// backend.rs:142-144 — the only writer of the "EWMA" field
pub fn set_latency_ns(&self, ns: u64) {
    self.latency_ns.store(ns, Ordering::Relaxed);
}
```

A repo-wide search for `alpha` / `decay` / `half_life` / `smoothing` finds no
latency-smoothing math anywhere in the workspace (the only `decay` hits are
`lb-h2`'s unrelated rapid-reset sliding window), and `latency_ns` has no second
writer. The stored value
is a **bare overwrite of the last sample**; `Ewma::pick` then multiplies it by
`active_connections + 1`. There is no exponential weighting, and — the part that
matters operationally — **no aging**: a stale sample never decays toward neutral.

**Concrete failure scenario** (assuming the field is wired, which is the stated
intent): backend `S` returns one unusually fast response (200 µs, e.g. a cached
404) and is then not selected for a minute. Its score stays `200 µs × 1` while
every busy peer carries a realistic 20 ms. `Ewma::pick` returns the strict minimum,
so `S` wins **every** pick until its next completed sample lands — and because the
score is recomputed from a single stale number rather than a decaying average, one
outlier sample pins the entire listener onto one backend. The reverse is equally
bad: one 5 s timeout sample banishes a healthy backend until it is next sampled,
and it can only be sampled if it is selected. Both are the classic reasons the
algorithm is specified as a *decaying* average with elapsed-time weighting
(Finagle/`P2C+peakEWMA` uses `w = exp(-Δt/τ)`).

The cold-start branch itself is correct and well-argued (`ewma.rs:25-36`:
unmeasured inherits the worst observed latency, or 1) and `u128` prevents the
multiply overflowing (`ewma.rs:42-49`). A backend with a genuine 0 ns measurement
is indistinguishable from cold — harmless.

This is **not** the documented "EWMA is unfed" item: that says the input is
missing; this says the algorithm behind the input is also absent, so feeding
`set_latency_ns` per request would **not** yield EWMA behaviour.

**Would an existing test catch it?** No. `tests/balancer_ewma.rs` and
`ewma.rs:61-112` only ever set the field directly and check ordering within one
`pick`. No test advances time or feeds a sequence. Additionally
`tests/balancer_ewma.rs:38` (`test_balancer_ewma_zero_latency`) is **vacuous with
respect to its name**: it asserts index 0 wins with `latency = [0, 1]`, which is
true both with the cold-start guard (scores 1 vs 1, first-min wins) and without it
(scores 0 vs 1) — deleting the guard leaves the test green. The in-crate
`test_cold_start_no_thundering_herd` (`ewma.rs:76`) is the real control.

---

## BAL-06 — every load-aware algorithm reads a snapshot that has no production writer AND atomics that have no production writer: least-connections / least-request / EWMA all return index 0 for every pick

Severity: **MEDIUM** · **LIBRARY-ONLY** · Blocking for prod: **NO**; blocking
for exposing any load-aware policy

Files: `crates/lb-balancer/src/lib.rs:34-102`, `crates/lb-core/src/backend.rs:85-133`,
`least_connections.rs:23-31`, `least_request.rs:23-31`, `p2c.rs:41`, `ewma.rs:48`.

The in-code comment already flags half of this
(`lib.rs:34-37`: "`sync_from_state` has NO production caller"), and `features.md`
says EWMA "would degrade to a load-based pick". Both understate it. The verified
call graph is:

* `BackendState::{inc,dec}_connections` and `{inc,dec}_requests` — **zero callers
  outside `lb-core`'s own unit tests** (`lb-core/src/lib.rs:55-72`). Confirmed by
  a workspace-wide grep; the only other `active_connections` in the binary
  (`main.rs:646,1639,3050,3244`) is a *different*, listener-scoped `AtomicU64`
  that is written and **never read**.
* `Backend::with_state` — no production caller. Both production constructions use
  `Backend::new` / a struct literal with `state: None`
  (`main.rs:1606`, `passthrough.rs:889`).
* `sync_from_state` — no production caller.

So in the running binary every `Backend.active_connections`,
`.active_requests` and `.latency_ewma_ns` is permanently `0`. Consequence for the
ten library algorithms if a policy key were added tomorrow:

| policy | actual behaviour with the current wiring |
|---|---|
| `LeastConnections` | `0 < u64::MAX` on `i = 0`, no later `<` ever true → **always index 0** |
| `LeastRequest` | **always index 0** |
| `Ewma` | all cold → equal scores → **always index 0** |
| `PowerOfTwoChoices` | ties resolve to `a` → uniform random (harmless but not P2C) |

That is not "degrades to least-connections" — it is a **100 % blackhole onto
`backends[0]`**, i.e. an N-fold capacity loss and a single point of failure, on
three of the ten policies.

Forward-looking requirement for the wiring increment (the brief's item 4): there
is currently **no RAII guard** for in-flight accounting, and the request
lifecycle has many exits that would each need one —
`h1_proxy.rs:1813/1817/1825/1834/1842` and `h2_proxy.rs:1038/1042/1050/1058/2122/
2133/2142` are the *recorded* exits alone, and the cancel paths (drain
`select!` at `main.rs:3204-3240`, client disconnect, watchdog abort) exit without
passing any of them. A counter incremented at pick time and decremented at those
sites will leak on the cancel paths, and a leaked increment on a
least-connections backend is permanent (`dec_connections` saturates at 0, so
compensation is impossible). The guard must be a `Drop` type owned by the request
future, created at pick time.

**Would an existing test catch it?** The unit/integration tests all set the
snapshot fields by hand, so they prove the *comparison* and nothing about the
*feed*. `crates/lb-balancer/tests/balancer_counter_sync.rs` does test
`sync_from_state` under concurrency — but it calls `inc_connections` and
`sync_from_state` itself, i.e. it tests a contract no production code
participates in.

---

## BAL-07 — the suppressed-ejection warning is emitted once per failed request: unbounded warn-rate log flood during a correlated outage

Severity: **LOW** · **LIVE-PATH** · Blocking for prod: **NO**

File: `crates/lb-health/src/ejection.rs:344-356`.

```rust
FailureAction::Suppressed { ejected, total } => {
    self.ejections_suppressed_total.fetch_add(1, Ordering::Relaxed);
    tracing::warn!(
        backend = %addr, ejected, total,
        min_healthy_percent = u32::from(self.inner.read().policy.min_healthy_percent),
        "passive health: ejection SUPPRESSED by the minimum-healthy floor — ...");
}
```

Once the floor is holding, an already-`Unhealthy` backend with
`ejected_until == None` re-enters the `None =>` arm on **every** subsequent
failure, so every failed request emits a `warn`. At 10 k rps of failing requests
that is 10 k warn lines/second, with a per-line `SocketAddr` Display and a second
lock acquisition (`self.inner.read()`) — precisely when the operator's log
pipeline is already saturated. Same class as the recorded
CF-S39-H3-REJECT-LOG-SPAM. The counter (`ejections_suppressed_total`) already
carries the signal; the log should be throttled the way
`passthrough.rs::audit_allow` throttles its audit lines (one per window).

Adjacent, non-blocking: that `self.inner.read()` executes *after* the write guard
is dropped at `ejection.rs:323`, which is correct — but `parking_lot::RwLock` is
not reentrant, so moving this log inside the decision block (an obvious future
"simplification") self-deadlocks the whole listener. Worth a comment.

**Would an existing test catch it?** No — no test asserts on log volume.

---

## BAL-08 — `inc_connections` documents an `AcqRel` → `Acquire` pairing that does not exist; the loom model tests orderings the code does not use, and never runs

Severity: **LOW** · **LIVE-PATH type (currently unreachable code)** ·
Blocking for prod: **NO**

File: `crates/lb-core/src/backend.rs:79-88` and
`crates/lb-balancer/tests/loom_atomic_counter.rs:5-33`.

```rust
// backend.rs:79-88
pub fn active_connections(&self) -> u64 {
    self.active_connections.load(Ordering::Relaxed)       // <-- Relaxed
}
/// ... `AcqRel`, unlike the `Relaxed` request counters,
/// because the SCHEDULER reads it to drive a pick — the asymmetry is deliberate, not an oversight.
pub fn inc_connections(&self) {
    // CLIPPY-OK: G-site. AcqRel publishes to the scheduler's paired Acquire load.
    self.active_connections.fetch_add(1, Ordering::AcqRel);
}
```

There is no paired `Acquire` load: the only reader loads `Relaxed`
(`backend.rs:80`), as does `dec_connections`' CAS (`backend.rs:92-100`) and
`Clone` (`backend.rs:156`). For a pure counter `Relaxed` on both sides is correct
and cheaper — the defect is the **comment asserting a guarantee the code does not
provide**, which is exactly what a future author would rely on when publishing
other state before `inc_connections`.

The loom model compounds it: `loom_atomic_counter.rs:18-24` models
`fetch_add(Release)` against `load(Acquire)` — neither of which is what the code
does. And `#![cfg(loom)]` with no `loom` lane in `.github/workflows/*` or
`scripts/` means it has never executed. Per the brief's "loom tests for any
lock-free code": the repo has a loom test that is both unrun and unfaithful.

Recommendation: either make the reader `Acquire` (matching the comment and the
loom model) or make the writer `Relaxed` and delete the claim. Do not leave the
three inconsistent.

**Would an existing test catch it?** No — x86 gives `Relaxed` the same observable
behaviour as `Acquire` for a counter, and the loom lane does not exist.

---

## BAL-09 — `with_thread_rng()` returns a balancer that cannot implement `LoadBalancer`

Severity: **LOW** · **LIBRARY-ONLY** · Blocking for prod: **NO**

Files: `crates/lb-balancer/src/p2c.rs:56-59`, `random.rs:31-34`,
`weighted_random.rs:46-49`.

```rust
// p2c.rs:20 — the impl bound
impl<R: Rng + Send + Sync> LoadBalancer for PowerOfTwoChoices<R> { ... }
// p2c.rs:56-59 — the helper
pub fn with_thread_rng() -> PowerOfTwoChoices<rand::rngs::ThreadRng> {
    PowerOfTwoChoices::new(rand::rng())
}
```

`rand` is pinned at 0.10 (`Cargo.toml:130`) and `ThreadRng` there is explicitly
neither `Send` nor `Sync`
(`rand-0.10.1/src/rngs/thread.rs:80`: "The handle cannot be passed between
threads (is not `Send` or `Sync`)"; it is an `Rc<UnsafeCell<..>>`). So all three
`with_thread_rng()` helpers return a value that does **not** satisfy the crate's
only balancer trait: it cannot be stored in a `Box<dyn LoadBalancer>`, cannot be
shared behind an `Arc`, and cannot cross a task boundary. Anyone wiring P2C /
Random / WeightedRandom must inject `StdRng`/`ChaCha` instead — and because
`pick` takes `&mut self`, the shared instance also needs a `Mutex`, serialising
every pick on the hot path. Worth deciding before exposure: a per-worker
thread-local balancer avoids the global lock.

**Would an existing test catch it?** No — the helpers are never called in any
test or in the binary, so the impl is simply never required. `cargo clippy`
cannot see it either; it only fails at the future use site.

---

## BAL-10 — no keyed algorithm can reach the L7 datapath: `BackendInfoPicker::pick_info` takes no request context, and `LbPolicy` has no consumer

Severity: **LOW (design)** · **LIBRARY-ONLY** · Blocking for prod: **NO**

Files: `crates/lb-l7/src/upstream.rs:67-70`, `crates/lb-core/src/policy.rs:7-30`,
`crates/lb-balancer/src/lib.rs:105-137`.

```rust
// upstream.rs:67-70 — no key, no request, no headers
pub trait BackendInfoPicker: Send + Sync {
    fn pick_info(&self) -> Option<UpstreamBackend>;
}
```

Three of the eleven algorithms (`Maglev`, `RingHash`, `SessionAffinity`)
implement `KeyedLoadBalancer::pick_with_key(&self, backends, key)` and therefore
cannot be expressed as a `BackendInfoPicker` at all — the trait gives the picker
nothing to hash. The two families also have no common object-safe supertype
(`LoadBalancer::pick(&mut self, &[Backend]) -> usize` vs
`KeyedLoadBalancer::pick_with_key(&self, …)`), and `LbPolicy` / `Cluster`
(`lb-core/src/{policy,cluster}.rs`) have **zero consumers anywhere in the
workspace** — nothing maps a policy value to a balancer. Exposing a policy key is
therefore not a config-schema change; it needs (a) a key source threaded into
`pick_info`, (b) a factory, (c) a decision on `&mut self` + `Mutex` vs
thread-local. Recording this so the exposure increment is scoped honestly.

---

## BAL-11 — consistent-hash keys are unsalted and publicly computable: an attacker who influences the key can pin all of their load onto one chosen backend

Severity: **LOW** · **LIBRARY-ONLY**, plus a knob-gated live variant ·
Blocking for prod: **NO**

Files: `crates/lb-balancer/src/session_affinity.rs:17-38`,
`crates/lb-quic/src/passthrough.rs:202-217` + `359-363`.

```rust
// session_affinity.rs:30-37
let h = Self::mix(key);
let idx = (h as usize) % backends.len();
```

`mix` is an unkeyed murmur3 finalizer and the backend count is public, so
`idx` is offline-computable. If the eventual key source is client-controlled (a
cookie, a header, a URL component — the usual sticky-session inputs), an attacker
grinds keys until `mix(key) % N == t` and lands **100 % of their traffic on
backend `t`**, concentrating a volumetric attack at `1/N` of the cost and
defeating the pool's load spreading. The same property holds for `Maglev` and
`RingHash` — all three hashes are unkeyed. Decide the API now: the key must be
salted with a **cluster-shared** secret (not per-process, or cross-instance
consistency dies with it — `retry_secret_path` is the existing precedent for a
shared-on-disk secret), or the key must be restricted to values the client cannot
choose.

Live variant: Mode A hashes the routing DCID (`passthrough.rs:360`). With the
default `mint_retry = true` this is the LB-minted `sample_lb_scid()` — server-chosen,
not steerable. With `mint_retry = false` (the documented trusted-network escape)
it is the **client's own DCID**, so a client can choose its backend. Bounded by
the knob being off by default and already-documented trusted-network framing.

Separate note on the same file: `SessionAffinity` is `hash % N`, not consistent
hashing — removing one backend of `N` remaps ~`(N−1)/N` of all sessions, not
`1/N`. The module comment says "stable only while the backend set is unchanged",
which is accurate; flagging only because the name invites the opposite assumption
and the doc-facing description in `features.md:97` lists it alongside the two
consistent-hash algorithms.

---

## BAL-12 — the floor counts distinct resolved addresses, not rotation slots

Severity: **LOW** · **LIVE-PATH** · Blocking for prod: **NO**

File: `crates/lb-health/src/ejection.rs:199-203` + `460-469`, caller
`crates/lb/src/main.rs:1584-1606`.

`HealthRegistry::new` keys a `HashMap<SocketAddr, Entry>` while the picker
rotation is a `Vec` of the same length as the config list. Two config entries
that resolve to the same address (two DNS names for one host — realistic, and
`main.rs:1591` takes only `lookup.first()`) collapse to one registry entry.
`total = inner.entries.len()` is then the **deduped** count: config
`[X, X, Y]` gives `total = 2`, `max_ejectable = 1`, so ejecting `X` removes
**2 of 3 rotation slots** (67 %) while the floor believed it had left 50 % in.
Contained by fail-open; recorded because the arithmetic is presented as a
guarantee. Fix direction: pass slot multiplicity, or dedupe the rotation.

**Would an existing test catch it?** No — every test uses distinct addresses.

---

## BAL-13 — test-coverage defects (a finding in its own right)

Severity: **INFO** · Blocking for prod: **NO**

1. **Disruption tests only ever touch the tail.** `maglev.rs:180`,
   `ring_hash.rs:165`, `tests/balancer_maglev.rs:29`,
   `tests/balancer_ring_hash.rs:26` all add or remove the *last* backend, so no
   survivor's index changes. The index-shift class (BAL-03) and the order class
   (BAL-04) are unreachable by the entire suite.
2. **Disruption bars are 50-60 % where the answer is ~100 %.** A table that
   randomly reassigned 40 % of surviving keys passes every one of them.
3. **Distribution bars tolerate a 4-5× skew.** `tests/balancer_maglev.rs:79` and
   `tests/balancer_ring_hash.rs:74` assert `count > 500` against an expected
   2000-2500. Maglev's fill is near-exactly equal by construction, so the bound
   could be ±5 % and still be robust.
4. **`tests/balancer_ewma.rs:38` is vacuous w.r.t. its name** — passes with and
   without the cold-start guard (see BAL-05).
5. **No concurrency test anywhere** for any picker, any balancer, or the health
   registry (BAL-02). Every e2e control is sequential.
6. **The loom test never runs and is unfaithful to the code** (BAL-08).
7. **No proptest / fuzz target for any balancer.** The two properties that want
   proptest are exactly the ones with no coverage: "removing any one backend
   remaps ≤ ~2/N of keys, for any subset" and "any permutation of the same set
   yields the same key→id mapping".
8. **Credit where due:** `crates/lb-l7/tests/health_ejection_e2e.rs` and
   `health_ejection_byte_identical.rs` are strong — per-backend accept counters, a
   real `H1Proxy` over real sockets, a named pre-fix behaviour per control, and an
   explicit non-vacuity arm (`divergence_is_detectable`) for the differential.
   Their two gaps are time (nothing sleeps past `base_ejection` while failures
   continue — BAL-01) and concurrency (BAL-02).

---

## Verified clean (checked, no finding)

Recorded so the next reviewer does not redo it:

* **`TABLE_SIZE = 65537` is prime** (Fermat F4), so `skip ∈ [1, M-1]` is always
  coprime with `M` and `(offset + c*skip) % M` enumerates all `M` slots.
  `maglev.rs:6,40-41`.
* **`Maglev::populate` cannot spin.** `filled` is checked before every claim and
  the outer loop breaks at `TABLE_SIZE`, so the inner probe always has an empty
  slot within one full cycle; `next[i] ≤ M`, so `c * skip ≤ 65537 × 65536` — no
  `usize` overflow on any 64-bit target.
* **`backends.len() > TABLE_SIZE`** does not panic: surplus backends simply never
  enter the table, and `pick_with_key` range-checks (`maglev.rs:124-128`).
* **Duplicate backend ids** in Maglev share a permutation and interleave
  correctly (each takes alternate slots on the same probe sequence).
* **`lb-l4-xdp::MaglevTable` validates primality** (`lib.rs:218`,
  `is_prime` at `lib.rs:185-203`), which is what prevents the composite-`table_size`
  **infinite loop** in its otherwise-duplicated `populate`. Test-only consumer
  (`tests/l4_xdp_maglev.rs`), consistent with ROUND8-L4-04.
* **Ring-hash wrap-around is correct.** `ring_hash.rs:93-102`: `Err(i)` with
  `i >= len` wraps to 0, otherwise `i` is the first point clockwise; `Ok(i)`
  returns the exact match. The off-by-one at the wrap point is not present. The
  ring is sorted once (`ring_hash.rs:41`, stable sort ⇒ deterministic tie order).
* **Smooth WRR is the real nginx algorithm** and its accumulator is bounded:
  `Σ current_weights == 0` is invariant (each pick adds `total` across the vector
  and subtracts `total` from one entry), so `|cw| < total_weight` — no overflow
  over any uptime. Weight 0 is never selected (strict `>` plus the invariant),
  matching nginx "down" semantics. `weighted_round_robin.rs:54-77`.
* **Degenerate sets are guarded in all eleven algorithms.** Every `pick` /
  `pick_with_key` returns `NoBackends` on an empty slice before any `%`, `/` or
  index; `WeightedRoundRobin`/`WeightedRandom` return `AllZeroWeight` before
  `random_range(0..0)`; `p2c` special-cases `len == 1` before
  `random_range(0..len-1)`; `WeightedRandom`'s `dart -= w` cannot underflow. The
  crate denies `unwrap`/`expect`/`panic`/`indexing_slicing`/`unreachable`
  (`lib.rs:2-11`), and no production balancer path can panic under
  `panic = "abort"`.
* **`can_eject` arithmetic** is correct at N=1 (never), N=2 (exactly one), N=3
  (floor(1.5)=1), N=4 (two), `min_healthy_percent = 0` (absolute floor still
  holds) and `> 100` (saturates to "never eject"). `backoff` caps at
  `max_ejection` and cannot overflow (`shift ≤ 16`, `checked_mul`).
* **No reload index/list race.** The L7 pickers own an immutable `Vec` and are
  replaced wholesale through `ArcSwap` (`main.rs:606-611`); the L4 path's
  `backends`/`addresses` pair is built in one lockstep loop
  (`main.rs:1599-1606`) and is never mutated; the passthrough table and its
  address vector are built together at spawn. An index can never be computed
  against one length and applied to another.
* **Ejection state machine internals**: `record_failure` resolves its whole
  decision under one write lock (no TOCTOU on the floor); `HealthChecker`
  thresholds clamp 0 → 1; `UpstreamErrorClass::ClientRequest` maps to
  `NotAttempted` rather than `Success`, so a client spraying malformed requests
  cannot reset a genuine failure streak (asserted at `ejection.rs:552-557`).
* **The FEED / FILTER table in `known-limitations.md` matches the code**: H1/H2
  feed via `record_health` (`h1_proxy.rs:1813-1842`, `h2_proxy.rs:1038-1058`),
  L4 feeds dial outcomes only (`main.rs:3300-3320`), gRPC collapses its result
  before returning, H3-front does neither (`conn_actor.rs:1210`).
* **`lb-core/src/authority.rs`** is correct for its stated contract; its one gap
  (unbracketed `::1` accepted) is pinned by a deliberate test at
  `authority.rs:131`. One un-pinned nit: `[::1]junk` passes, because
  `port_suffix` only validates what follows `]:` (`authority.rs:59-61`). No
  routing impact here (selection is not host-based) — flagged to the smuggling
  reviewer rather than claimed as a balancer finding.
* **Duplication note (R12), not a defect:** three round-robin implementations
  (`lb_balancer::RoundRobin`, `upstream::RoundRobinUpstreams`,
  `h1_proxy::RoundRobinAddrs`) and two Maglev implementations
  (`lb_balancer::maglev`, `lb_l4_xdp::MaglevTable`) coexist. Only
  `RoundRobinUpstreams` (L7) and `ListenerState.balancer` (L4) are live.
