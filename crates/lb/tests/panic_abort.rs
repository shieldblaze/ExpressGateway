//! CODE-2-02 proof — release-build smoke test that asserts a `panic!()` inside a tokio task aborts
//! the process when built under `panic = "abort"`.

use std::path::Path;

#[test]
fn release_profile_has_panic_abort() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_root = Path::new(manifest_dir)
        .ancestors()
        .nth(2)
        .expect("workspace root above crates/lb");
    let cargo_toml = workspace_root.join("Cargo.toml");
    let body = std::fs::read_to_string(&cargo_toml)
        .unwrap_or_else(|e| panic!("read {}: {e}", cargo_toml.display()));

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

/// Dynamic proof.
#[test]
#[cfg(unix)]
fn panic_in_tokio_task_aborts_release_process() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

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

    let build = Command::new(&cargo)
        .args(["build", "--release", "--quiet"])
        .current_dir(&tmp)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("invoke cargo build");
    if !build.status.success() {
        eprintln!(
            "skip: probe build failed (likely offline runner)\nstdout:{}\nstderr:{}",
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        );
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    let bin = tmp.join("target/release/code202_panic_probe");
    let run = Command::new(&bin).output().expect("invoke probe");

    assert_ne!(
        run.status.code(),
        Some(7),
        "CODE-2-02 regression: tokio task panic did NOT abort the process. \
         The probe printed the survived sentinel.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

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
