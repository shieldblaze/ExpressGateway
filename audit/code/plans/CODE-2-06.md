# Plan for CODE-2-06 + REL-2-10 — accept() error backoff
Finding-ref:     CODE-2-06 / REL-2-10 (critical, Open)
Files touched:
  - `crates/lb/src/main.rs`                         (accept loop classifier)
  - `crates/lb-l7/src/accept_backoff.rs`            (NEW — pure classifier)
  - `crates/lb-observability/src/metrics.rs`        (`accept_errors_total{kind}` — rel-owned)

Approach:
Replace the unconditional `continue` on accept error with a classifier
that maps `errno` to one of three actions: `Retry`, `Backoff(Duration)`,
or `Fatal`.

Step 1 — Classifier crate-local. New
`crates/lb-l7/src/accept_backoff.rs`:
```rust
use std::time::Duration;
pub enum AcceptAction { Retry, Backoff(Duration), Fatal }

pub struct AcceptBackoff {
    next: Duration,
    cap:  Duration,
}
impl AcceptBackoff {
    pub fn new() -> Self { Self { next: Duration::from_millis(5), cap: Duration::from_secs(1) } }
    pub fn reset(&mut self) { self.next = Duration::from_millis(5); }
    pub fn classify(&mut self, err: &std::io::Error) -> AcceptAction {
        match err.raw_os_error() {
            Some(libc::EMFILE) | Some(libc::ENFILE) | Some(libc::ENOBUFS) | Some(libc::ENOMEM) => {
                let d = self.next;
                self.next = (self.next * 2).min(self.cap);
                AcceptAction::Backoff(d)
            }
            Some(libc::ECONNABORTED) | Some(libc::EINTR) | Some(libc::EPROTO) => AcceptAction::Retry,
            _ => AcceptAction::Fatal,
        }
    }
    pub fn kind(err: &std::io::Error) -> &'static str {
        match err.raw_os_error() {
            Some(libc::EMFILE) => "emfile",
            Some(libc::ENFILE) => "enfile",
            Some(libc::ENOBUFS) => "enobufs",
            Some(libc::ENOMEM) => "enomem",
            Some(libc::ECONNABORTED) => "econnaborted",
            Some(libc::EINTR) => "eintr",
            _ => "other",
        }
    }
}
```

Step 2 — Accept loop edit. `crates/lb/src/main.rs:1099–1106` becomes:
```rust
let mut backoff = AcceptBackoff::new();
loop {
    let res = tokio::select! {
        biased;
        _ = state.shutdown.token().cancelled() => break,
        r = listener.accept() => r,
    };
    let (client_stream, client_addr) = match res {
        Ok(c) => { backoff.reset(); c }
        Err(e) => {
            metrics::accept_errors_total().with_label_values(&[AcceptBackoff::kind(&e)]).inc();
            match backoff.classify(&e) {
                AcceptAction::Retry => continue,
                AcceptAction::Backoff(d) => {
                    tracing::warn!(error = %e, backoff_ms = d.as_millis() as u64,
                                   "accept fd-exhaust; backing off");
                    tokio::select! {
                        _ = state.shutdown.token().cancelled() => break,
                        _ = tokio::time::sleep(d) => continue,
                    }
                }
                AcceptAction::Fatal => {
                    tracing::error!(error = %e, "fatal accept error; listener exiting");
                    return Err(e.into());
                }
            }
        }
    };
    // … existing accept body …
}
```

Step 3 — Log rate limiting. Single `tracing::warn!` per backoff step
caps emission at one log per backoff iteration (≤1 / cap-second once
saturated). No tracing-subscriber filter changes needed; the
arithmetic-progression of sleeps self-rate-limits.

Step 4 — `rel` exports `accept_errors_total{kind=...}`; `code`
publishes the call site. The label set is closed (the `kind()` helper
above) to bound cardinality.

Proof:
- `crates/lb-l7/tests/accept_backoff.rs::backoff_doubles_to_cap`:
  unit test on the classifier — feed 10 successive EMFILEs, assert
  durations are 5, 10, 20, 40, 80, 160, 320, 640, 1000, 1000 ms.
- `crates/lb/tests/emfile_accept.rs::no_busy_loop_on_emfile`: gated
  on `cfg(target_os = "linux")`; sets `ulimit -n` via
  `setrlimit(RLIMIT_NOFILE, 64)`, opens 64 sockets, attempts a 65th
  accept. Asserts (a) loop emits exactly one `warn!` per backoff step
  in a 200 ms window (NOT busy-spinning), (b) `accept_errors_total
  {kind="emfile"}` counter went up by ≥1, (c) once an fd is released,
  the next accept succeeds and backoff resets.
- `crates/lb/tests/emfile_accept.rs::fatal_error_returns`: inject a
  synthetic non-classified error and assert the listener task exits
  with that error.

Risk / blast radius:
- Slightly slower recovery from a single ECONNABORTED burst is
  acceptable; the classifier short-circuits that branch with `Retry`
  (no sleep).
- The cap of 1 s prevents long stalls during transient fd pressure.
- Cancel-aware sleep ensures drain still works while in backoff.

Cross-ref:    REL-2-10 (metric + alert + runbook), CODE-2-03 (cancel arm)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
