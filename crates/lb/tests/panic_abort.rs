//! CODE-2-02 proof — release-build smoke test that asserts a `panic!()`
//! inside a tokio task aborts the process when built under
//! `panic = "abort"`.
//!
//! Strategy: we don't have a "panic on demand" hook in the
//! `expressgateway` binary, so we test the profile policy directly.
//!
//! Two assertions:
//!
//! 1. **Static**: the workspace `Cargo.toml` `[profile.release]` block
//!    has `panic = "abort"`. This is the line CODE-2-02 introduced;
//!    if a future commit reverts it without updating this test the
//!    failure mode is loud.
//! 2. **Dynamic**: spawn a tiny ad-hoc helper that builds + runs a
//!    one-line program with `panic = "abort"` in its profile, panics
//!    from inside a tokio task, and asserts the child died with
//!    SIGABRT (exit code 134 on Linux, or a `code() == None` +
//!    signal == ABRT on Unix). Skipped when `cargo` is not available
//!    on the test host (e.g. some sandboxed CI runners).
//!
//! The Prometheus counter assertion (panic_total += 1) is deferred to
//! rel REL-2-07 / REL-2-15 in Wave 2 — see the comment on
//! `PANIC_TOTAL` in `lb/src/main.rs`.

use std::path::Path;

#[test]
fn release_profile_has_panic_abort() {
    // Walk up from CARGO_MANIFEST_DIR (crates/lb) to the workspace root.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = Path::new(manifest_dir)
        .ancestors()
        .nth(2)
        .expect("workspace root above crates/lb");
    let cargo_toml = workspace_root.join("Cargo.toml");
    let body = std::fs::read_to_string(&cargo_toml)
        .unwrap_or_else(|e| panic!("read {}: {e}", cargo_toml.display()));

    // Crude but resilient: find the `[profile.release]` header, then
    // require a `panic = "abort"` line before the next `[` section.
    let start = body
        .find("[profile.release]")
        .expect("[profile.release] block must exist");
    let tail = &body[start..];
    let end = tail[1..].find('[').map_or(tail.len(), |off| off + 1);
    let section = &tail[..end];
    assert!(
        section.contains("panic = \"abort\""),
        "CODE-2-02 regression: [profile.release] missing `panic = \"abort\"`:\n{section}"
    );
}

/// Dynamic proof. Compiles + runs a tiny throwaway crate in release
/// mode with `panic = "abort"` and a tokio runtime, panics from
/// inside a `tokio::spawn`'d task, asserts the child dies via SIGABRT
/// (Unix) / exit code 0x80000003 (Windows-equivalent — not run).
///
/// Skipped when:
///   - `cargo` is not on PATH (sandboxed runner without rustup).
///   - The test is running under miri / wasm.
#[test]
#[cfg(unix)]
fn panic_in_tokio_task_aborts_release_process() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    // Locate cargo. If the test host lacks it, skip (the static test
    // above is still a hard regression gate).
    let cargo = match std::env::var("CARGO") {
        Ok(c) => c,
        Err(_) => match Command::new("cargo").arg("--version").output() {
            Ok(o) if o.status.success() => "cargo".to_owned(),
            _ => {
                eprintln!("skip: cargo not available on PATH");
                return;
            }
        },
    };

    // Build a throwaway crate in a temp dir so its target/ never
    // contaminates the workspace target/.
    let tmp = std::env::temp_dir().join(format!("code-2-02-panic-abort-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("src")).expect("mkdir tmp");

    let cargo_toml = r#"
[package]
name = "code202_panic_probe"
version = "0.0.1"
edition = "2024"
publish = false

[[bin]]
name = "code202_panic_probe"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }

[profile.release]
opt-level = 0
panic = "abort"
codegen-units = 1
"#;
    let main_rs = r#"
fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let h = tokio::spawn(async {
            panic!("CODE-2-02 probe: panic-from-task must abort");
        });
        let _ = h.await; // under unwind this would return JoinError::Panic; under abort the process dies first
        eprintln!("ERROR: process survived panic — abort policy not in effect");
        std::process::exit(7);
    });
}
"#;
    std::fs::write(tmp.join("Cargo.toml"), cargo_toml).expect("write Cargo.toml");
    std::fs::write(tmp.join("src/main.rs"), main_rs).expect("write main.rs");

    // Build release.
    let build = Command::new(&cargo)
        .args(["build", "--release", "--quiet"])
        .current_dir(&tmp)
        // Drop any inherited CARGO_TARGET_DIR so cargo writes the probe
        // to <tmp>/target (current_dir/target), matching the run step
        // below and keeping it out of the shared workspace target/.
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("invoke cargo build");
    if !build.status.success() {
        eprintln!(
            "skip: probe build failed (likely offline runner)\nstdout:{}\nstderr:{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        );
        // Don't fail — the static test is the hard gate; this dynamic
        // test is a smoke gate that requires network for crates.io.
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    let bin = tmp.join("target/release/code202_panic_probe");
    let run = Command::new(&bin).output().expect("invoke probe");

    // Under `panic = "abort"` the child must die with SIGABRT (signal 6),
    // not exit cleanly and not return exit code 7 (which is the
    // "survived" sentinel). Exit code 7 means panic did not abort —
    // that is the regression case.
    assert_ne!(
        run.status.code(),
        Some(7),
        "CODE-2-02 regression: tokio task panic did NOT abort the process. \
         The probe printed the survived sentinel.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    // On Unix the abort manifests as termination by SIGABRT (signal 6).
    let signal = run.status.signal();
    assert_eq!(
        signal,
        Some(6),
        "CODE-2-02 regression: expected SIGABRT (signal 6), got status={:?} signal={:?}\nstdout: {}\nstderr: {}",
        run.status.code(),
        signal,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
