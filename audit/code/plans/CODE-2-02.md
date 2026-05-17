# Plan for CODE-2-02 + REL-2-15 — panic = "abort" + set_hook
Finding-ref:     CODE-2-02 / REL-2-15 (critical, merged, Open)
Files touched:
  - `Cargo.toml`                                    (workspace `[profile.release]`)
  - `crates/lb/src/main.rs`                         (`install_panic_hook()` early in `main`)
  - `crates/lb-observability/src/panic.rs`          (NEW — hook impl, log + abort)

Approach:
Two coupled changes: (a) make release builds abort on panic so unsafe
boundaries don't unwind into half-restored invariants; (b) install a
panic hook that logs at `error!` with backtrace, then aborts.

Step 1 — Profile flags. Edit workspace `Cargo.toml`:
```toml
[profile.release]
opt-level     = 3
lto           = "thin"
codegen-units = 1
strip         = "symbols"
panic         = "abort"   # NEW

[profile.bench]
inherits = "release"

# dev/test KEEP unwind (default) — required for proptest minimisation
# and loom interleaving exploration. Do NOT set panic for [profile.dev]
# or [profile.test]; rustc's default is "unwind" which is what we want.
```
Note: `panic = "abort"` in release means `std::panic::catch_unwind`
in release will treat panics as aborts. We have verified zero
`catch_unwind` call sites (cf. CODE-2-08's recommendation §1 must be
re-evaluated: under `panic = "abort"` the AssertUnwindSafe wrapper is
moot — CODE-2-08 plan §1 is downgraded to "process death is the safer
default; cleanup logic redundant").

Step 2 — Panic hook. New file
`crates/lb-observability/src/panic.rs`:
```rust
use std::backtrace::Backtrace;
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let bt = Backtrace::force_capture();
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line()))
                                       .unwrap_or_else(|| "<unknown>".into());
        let payload  = info.payload_as_str().unwrap_or("<non-string>");
        tracing::error!(
            target: "panic",
            location = %location,
            payload  = %payload,
            backtrace = %bt,
            "process panic — aborting"
        );
        // Give the subscriber a chance to flush.
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::process::abort();
    }));
}
```
And in `crates/lb/src/main.rs` `async_main` (or `main` before runtime
construction):
```rust
lb_observability::panic::install();
```
Inserted as line ~1 of `main` so any panic during runtime construction
is also captured.

Step 3 — Tests. `crates/lb-observability/tests/panic_hook.rs` uses
`std::process::Command` to spawn the test binary in a subprocess (so
the abort doesn't kill the test harness) and asserts:
1. stderr contains the panic location string.
2. exit status reflects SIGABRT (status code 134 on Linux).
3. Dev profile of the test still unwinds (catch_unwind around a
   helper panics succeeds without aborting).

Proof:
- `cargo test -p lb-observability --test panic_hook` — covers hook
  behaviour and dev-profile unwind escape hatch.
- `cargo build --release && readelf -p .comment target/release/expressgateway`
  smoke-checks the profile applied.
- CI grep guard: `! grep -E '^panic\s*=' Cargo.toml | grep -v abort`
  in `[profile.release]` block (encoded as `tests/cargo_profile.rs`
  parsing `cargo metadata`).

Risk / blast radius:
- `panic = "abort"` changes failure mode: previously a panicked task
  was silently caught; now the process dies and systemd restarts it.
  Operators must ensure `Restart=on-failure` is set (rel REL-2-01
  runbook umbrella documents this).
- Binary size: -3 % typical (landing-pad elision).
- Loom and proptest still run under dev profile and keep unwind, so
  CODE-2-11 work is unaffected.
- One known caller, `CODE-2-08`, becomes a no-op under abort; that
  plan's §1 is dropped (cross-ref noted in CODE-2-08.md).

Cross-ref:    REL-2-15 (merged), CODE-2-08 §1 (now redundant),
              sec §B.2 / F-22 (Round 1 cross-review framing)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
