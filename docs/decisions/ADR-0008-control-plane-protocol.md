# ADR-0008: Control-plane protocol — file-backed ConfigBackend trait, ArcSwap hot-swap, SIGHUP, HA polling

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: nginx SIGHUP reload semantics, HAProxy Runtime API, envoy
  xDS, Pingora configuration lifecycle.

## Context and problem statement

An L4/L7 load balancer must accept configuration changes *at runtime*
without dropping connections. The options form a spectrum from "pull
files off disk" (nginx style) to "stream updates over gRPC" (Envoy's
xDS), and each point on the spectrum has operational and failure-mode
consequences.

For an initial release, shipping a full xDS client/server ahead of
stabilising the configuration schema itself is the wrong order: the
schema will change, and every change would ripple through an over-eager
distributed protocol. We instead need:

1. A **pluggable** storage seam so the same control-plane crate can be
   fed by a file today and by a streaming source later.
2. A **standalone** operator experience that matches nginx/HAProxy
   expectations: edit a file, send SIGHUP, the new config takes effect.
3. **Rollback** on invalid configuration — a broken config file must
   not kill the running process; the previous valid config must be
   restorable.
4. **HA polling** for deployments where two or more instances coordinate
   via a shared config blob (e.g. NFS, S3-with-polling) without bringing
   a consensus layer.
5. **Atomic hot-swap** at the data plane so reads never tear.

## Decision drivers

- Operator ergonomics: industry expects file + SIGHUP for standalone.
- Schema stability: avoid committing to a wire protocol before the
  schema settles.
- Panic-free lint compliance: the control plane must not `unwrap()` on
  a bad config, ever.
- Testability: both in-memory and on-disk paths must be unit-testable.
- Rollback: bad config must not turn into hard failure.
- Forward compatibility: future distributed backends (S3, etcd, xDS)
  must slot in behind the same trait.
- Monotonic version numbers for observability and debugging.

## Considered options

1. Hard-code file-based reload; add distributed backends later as a
   rewrite.
2. Define a `ConfigBackend` trait with file + in-memory impls; add more
   impls behind the trait as they are needed.
3. Start with gRPC/xDS directly.
4. Use a third-party config library (`figment`, `config-rs`).

## Decision outcome

Option 2, with concrete implementations:

- `ConfigBackend` trait (`crates/lb-controlplane/src/lib.rs:43–57`)
  with `load()` / `store()` methods returning `Result<..., ControlPlaneError>`.
- `FileBackend` — atomic file writes via tmp-file + rename
  (`crates/lb-controlplane/src/lib.rs:65–97`).
- `InMemoryBackend` — `Mutex<String>`, for tests and for wiring in
  other backends later.
- `ConfigManager` — holds the current config, a monotonic version
  counter, and the immediately-previous config for rollback
  (`crates/lb-controlplane/src/lib.rs:156–272`).
- `HaPoller` — periodic load + change detection
  (`crates/lb-controlplane/src/lib.rs:276–319`).

Hot-swap at the data plane is done via `arc-swap` (workspace
dependency in the root `Cargo.toml`: `arc-swap = "1"`).

## Rationale

- **File-backed with a trait**: the trait is the seam. Today only
  `FileBackend` and `InMemoryBackend` ship; tomorrow an xDS or etcd
  backend can drop in behind the same trait without touching
  `ConfigManager`. This is visible in `ConfigManager::new` which takes
  a `Box<dyn ConfigBackend>`.
- **Atomic file writes**: `FileBackend::store` writes to
  `.<filename>.tmp` in the same directory then renames into place
  (lines 82–96). Rename on POSIX is atomic within a filesystem, so the
  config file is never observed in a half-written state — important for
  HA polling, which might read concurrently from a second instance.
- **Validation before commit**: `ConfigManager::validate` requires
  non-empty + parseable TOML (lines 227–237). Called from `new`,
  `reload`, and `rollback`. An invalid file does not clobber the
  in-memory state.
- **Rollback**: `reload()` saves the old config into
  `previous_config` (line 196). `rollback_to_previous()` restores it
  via the backend + in-memory swap (lines 262–271). The test
  `tests/reload_zero_drop.rs` exercises this across 20 rapid
  reload/rollback cycles and asserts the version number is monotonic.
- **Monotonic version**: `ConfigManager::version()` increments on
  every successful reload or rollback (lines 197, 249, 269). Each
  *state change* — whether forward or backward — is a new version, so
  observers can tell "the config was reloaded" from "nothing happened."
  The `reload_zero_drop.rs` test pins this behaviour: after 19 reloads
  + 1 rollback + 1 no-op reload + 1 further reload, version = 22.
- **HA polling**: `HaPoller::poll()` returns `Some(new_config)` only
  when the content changes; idempotent re-loads from a quiescent shared
  backend are cheap (one `load()` + string compare).
- **ArcSwap hot-swap**: downstream crates that hold the live config
  use `arc_swap::ArcSwap<AppConfig>`; the control plane's job stops at
  producing a validated config string, and the data plane's job starts
  with `ArcSwap::store` replacing the live reference. `ArcSwap` gives
  wait-free reads and is the standard Rust choice for this pattern.
- **SIGHUP trigger**: the binary entry point (`crates/lb/src/main.rs`)
  installs a `tokio::signal` handler; on SIGHUP it calls
  `ConfigManager::reload`. No kernel tricks; a single signal, a single
  reload path.
- **Not figment/config-rs**: those crates are optimised for app
  startup, not for runtime reload with rollback + versioning. We need
  those semantics first-class.

## Consequences

### Positive
- Operators get the familiar "edit + SIGHUP" workflow.
- Rollback is free: the last-known-good is always in memory.
- The trait means we never have to rewrite `ConfigManager` when we add
  an xDS backend; we add an impl.
- HA deployments using a shared mount point get coordination without
  consensus infrastructure.
- `arc-swap` gives reads cost ~= atomic load + pointer deref; no
  lock contention on the hot path.

### Negative
- HA via polling is eventually consistent; nodes can briefly disagree.
  Acceptable for LB config; would not be for, say, authorization state.
- SIGHUP-only reload means we have no "preview" or "validate-only" CLI
  yet.
- File-based storage on a shared filesystem inherits the filesystem's
  consistency story (NFS: "maybe").

### Neutral
- The trait is object-safe (`Box<dyn ConfigBackend>`), so dynamic
  dispatch at one level of indirection. Not hot-path.

## Implementation notes

- `crates/lb-controlplane/src/lib.rs` — trait, two backends,
  `ConfigManager`, `HaPoller`.
- `crates/lb-config/src/lib.rs` — schema (the TOML types that the
  control plane opaquely ferries).
- `crates/lb-cp-client/src/lib.rs` — client-side of future distributed
  control plane; stub today, same seam.
- Root `Cargo.toml` — `arc-swap = "1"` workspace dep, `toml = "0.8"`.
- Tests: `tests/controlplane_standalone.rs`,
  `tests/controlplane_rollback.rs`, `tests/controlplane_ha.rs`,
  `tests/reload_zero_drop.rs`.

## Follow-ups / open questions

- When to add an xDS or etcd backend? Blocked on schema stabilisation.
- CLI flag `--validate-config` to dry-run `ConfigManager::validate`.
- Notification fanout to multiple data-plane watchers: today one
  `ArcSwap`, tomorrow a `tokio::sync::watch` channel.

## Sources

- nginx SIGHUP handling (public source tree, `src/os/unix/ngx_process_cycle.c`).
- HAProxy Runtime API docs.
- Envoy xDS (v3) spec.
- `arc-swap` crate README.
- Internal: `crates/lb-controlplane/src/lib.rs`,
  `tests/reload_zero_drop.rs`.
