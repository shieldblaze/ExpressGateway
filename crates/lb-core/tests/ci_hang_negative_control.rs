//! R13 negative control for the CI hang-as-failure gate (S46 / CF-S44).
//!
//! A timeout nobody has watched fire is a claim, not a gate. These two tests wedge on
//! demand so that `.config/nextest.toml`'s `slow-timeout.terminate-after` can be
//! observed KILLING a test and reporting it as a FAILURE — rather than escalating a
//! `SLOW` marker forever, which is what actually happened in run `30755744744` job
//! `91517507600` (5 h 06 m, cancelled by hand, never red).
//! Background: `audit/ci/s44-grpc-h3-te-trailer-hang.md`.
//!
//! # Inert unless asked
//!
//! Both tests return immediately unless `EG_CI_HANG_PROBE` is set, so in the normal
//! gate they pass in ~0 s and no existing test is skipped, slowed or weakened. They are
//! deliberately NOT `#[ignore]`d: `--run-ignored` would be a second switch to forget,
//! and an ignored test is easy to leave permanently un-run.
//!
//! # Running it
//!
//! Mechanism, ~40 s (uses the fast `hang-probe` profile):
//! ```text
//! EG_CI_HANG_PROBE=1 cargo nextest run -P hang-probe \
//!   -p lb-core --test ci_hang_negative_control
//! ```
//! Expect `TIMEOUT [  20.000s]` for both tests and exit code 100.
//!
//! The shipped 1200 s constant, ~20 min (default profile) — the probe above proves the
//! mechanism but not the number:
//! ```text
//! EG_CI_HANG_PROBE=1 cargo nextest run -p lb-core --test ci_hang_negative_control
//! ```
//!
//! # Two things this does NOT prove
//!
//! 1. It does not reproduce CF-S44's mechanism. U5 (untimed `SUITE_SERIAL.lock().await`
//!    vs an unbounded inner await) stays open. This is containment, not a fix.
//! 2. It does not prove the Coverage JOB goes red, because `--ignore-run-fail` sits
//!    between nextest's exit code and the job's conclusion. Only running the Coverage
//!    job with `EG_CI_HANG_PROBE=1` crosses that, and that is what the workflow's
//!    "Fail on a TERMINATED test" step exists for.
//!
//! # Warning
//!
//! The `Test` job runs `cargo test`, NOT nextest, so `.config/nextest.toml` does not
//! apply there. Setting `EG_CI_HANG_PROBE` in that job produces a genuine unbounded
//! hang, bounded only by that step's own `timeout-minutes`.

/// Returns `true` only when the probe has been explicitly armed.
///
/// Presence is the switch, not the value — `EG_CI_HANG_PROBE=0` still arms it, which is
/// intentional: anyone who sets the variable at all means to wedge something.
fn armed() -> bool {
    std::env::var_os("EG_CI_HANG_PROBE").is_some()
}

/// Wedges on an untimed `.await`, the same shape as the CF-S44 candidate mechanism:
/// a parked tokio runtime with no waker that will ever fire.
#[tokio::test]
async fn ci_hang_negative_control_async() {
    if !armed() {
        return;
    }
    eprintln!(
        "EG_CI_HANG_PROBE armed: awaiting a future that never resolves — expect a nextest TIMEOUT"
    );
    std::future::pending::<()>().await;
    unreachable!("nextest's terminate-after must fire before this line is reachable");
}

/// Wedges the OS thread instead. Proves the kill does not depend on an async runtime
/// cooperating, and covers a hang that cannot be blamed on tokio at all.
#[test]
fn ci_hang_negative_control_blocking() {
    if !armed() {
        return;
    }
    eprintln!("EG_CI_HANG_PROBE armed: sleeping forever — expect a nextest TIMEOUT");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
