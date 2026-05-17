# Plan for CODE-2-13 — Wire lb-controlplane (file-backed) + lb-health (passive read)
Finding-ref:     CODE-2-13 (medium, Open) — lead decision §E.6 in synthesis r2
Files touched:
  - `crates/lb/src/main.rs`                         (wire-up + SIGHUP handler)
  - `crates/lb-controlplane/src/file.rs`            (NEW — file-backed source)
  - `crates/lb-controlplane/src/lib.rs`             (re-export)
  - `crates/lb-health/src/lib.rs`                   (passive `is_healthy` reader)
  - `audit/deferred.md`                             (record distributed CP deferral)

Approach:
Lead has split this finding into "required slice" (file-backed CP +
passive health) and "deferred" (distributed CP backends). This plan
implements *only* the required slice.

Step 1 — File-backed control plane. New
`crates/lb-controlplane/src/file.rs`:
```rust
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;

pub struct FileConfigSource {
    path: PathBuf,
    tx: watch::Sender<Arc<lb_config::Config>>,
}
impl FileConfigSource {
    pub fn load_initial(path: PathBuf) -> Result<(Self, watch::Receiver<Arc<lb_config::Config>>)> {
        let cfg = Arc::new(parse_config_file(&path)?);
        let (tx, rx) = watch::channel(cfg);
        Ok((Self { path, tx }, rx))
    }
    /// Called on SIGHUP. Re-reads the file, validates, only commits if valid.
    pub fn reload(&self) -> Result<()> {
        let new = Arc::new(parse_config_file(&self.path)?);
        lb_config::validate::reloadable(&new, &self.tx.borrow())?;
        let _ = self.tx.send(new); // ignore no-receiver
        Ok(())
    }
}
```
`reloadable` is a new validation pass in `lb-config::validate` that
checks only knobs that may change at runtime (not bind addresses,
listener modes, or TLS configuration); these get rejected on hot
reload (operator must restart).

Step 2 — Bind into `main.rs`. After `parse_config`:
```rust
let (config_source, config_rx) = FileConfigSource::load_initial(config_path)?;
// Listeners and pickers take `config_rx.clone()`; they `borrow()` for hot reads.
```
SIGHUP handler (joins the existing signal-handler set):
```rust
let mut sighup = signal(SignalKind::hangup())?;
let cs = Arc::new(config_source);
let shutdown = shutdown.clone();
shutdown.tracker().spawn({
    let cs = Arc::clone(&cs);
    async move {
        loop {
            tokio::select! {
                _ = shutdown.token().cancelled() => break,
                _ = sighup.recv() => {
                    match cs.reload() {
                        Ok(()) => tracing::info!("config reloaded"),
                        Err(e)  => tracing::error!(error = ?e, "config reload rejected"),
                    }
                }
            }
        }
    }
});
```

Step 3 — Passive health reader. `lb-health::HealthChecker` already
maintains a per-backend healthy flag; today nothing reads it. Wire it
into the picker:
```rust
// crates/lb-balancer/src/picker.rs
fn select(&self, backends: &[Arc<BackendState>]) -> Option<&BackendState> {
    backends.iter()
            .filter(|b| b.is_healthy())   // <— new gate
            .min_by_key(|b| b.active_connections.load(Ordering::Acquire))
}
```
The `is_healthy()` source today is the *passive* signal — i.e. set on
connect / read failure in `lb-io::pool` error-handling path. This
plan adds NO active probing loop; that is owned by REL-2-05.

REL-2-05 will *layer* an active-probe loop atop the same
`BackendState::is_healthy` field on top of this scaffolding.

Step 4 — `deferred.md` update. Append:
```
- Distributed control-plane backends (etcd/consul/xDS): deferred from
  Round 3. Reason: lead decision §E.6 — out of audit scope. Re-open
  when a customer requirement materialises.
- Active health-check loop: required slice retained (REL-2-05); the
  current plan provides only the passive gate.
```

Insertion points published for layering:
- `rel` (REL-2-05) lands the active-probe `HealthCheckLoop` task in
  its own file `crates/lb-health/src/active.rs`, spawned from
  `main.rs` after this plan's wire-up, into the same `Shutdown`
  tracker.

Proof:
- `crates/lb-controlplane/tests/file_reload.rs::sighup_reloads_config`:
  spawn binary in test harness, write new TOML, send SIGHUP, observe
  the watch channel receives the new value and the picker reflects
  new weights.
- `crates/lb-controlplane/tests/file_reload.rs::invalid_reload_rejected`:
  write TOML with a non-reloadable change (bind addr); SIGHUP; assert
  error logged + old config still active.
- `crates/lb-balancer/tests/picker_health.rs::unhealthy_backend_skipped`:
  mark backend unhealthy; picker returns the other; mark healthy;
  picker includes it again.

Risk / blast radius:
- SIGHUP semantics: must be idempotent. Two rapid SIGHUPs ≤ one reload
  if file unchanged. Tested.
- Watch-channel semantics: receivers may miss intermediate values
  under rapid updates; for config that's correct behaviour
  (last-write-wins). Documented.
- Reloadable-knob list must be conservative; add knobs only after
  manual review. Documented in `docs/decisions/reloadable.md`.

Cross-ref:    REL-2-05 (active probing layered atop),
              CODE-2-03 (`Shutdown` token joins SIGHUP task)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
