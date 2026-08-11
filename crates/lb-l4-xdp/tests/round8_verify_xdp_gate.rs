//! ROUND8-L4-10 proof: `scripts/verify-xdp.sh` exits with the documented codes and the shipped
//! baselines carry the `HARNESS-CAPTURED-PENDING-CI-RERUN` marker until the first CI refresh.
//! These tests assert the gate POSTURE, never invoking docker; CI exercises the matrix itself.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root canonicalize")
}

fn script_path() -> PathBuf {
    repo_root().join("scripts").join("verify-xdp.sh")
}

fn baseline_path(kver: &str) -> PathBuf {
    repo_root()
        .join("audit")
        .join("ebpf")
        .join("verifier-logs")
        .join(format!("{kver}.log.committed"))
}

#[test]
fn baseline_files_exist_for_all_supported_kernels() {
    // Audit-of-audit posture: every supported kernel must have SOME committed baseline file,
    // even a placeholder. The previous round shipped the script with none of them.
    for kver in &["5.15", "6.1", "6.6"] {
        let p = baseline_path(kver);
        assert!(
            p.exists(),
            "missing verifier-log baseline for kernel {kver} at {p:?}",
        );
        let body = fs::read_to_string(&p).expect("read baseline");
        assert!(
            !body.is_empty(),
            "empty baseline at {p:?} — the previous-round failure mode",
        );
    }
}

#[test]
fn placeholder_baselines_carry_pending_marker() {
    // While the baselines are placeholders they MUST self-identify as such — the marker is how
    // doc-lint and the first CI refresh tell a real baseline from one needing refresh.
    for kver in &["5.15", "6.1", "6.6"] {
        let body = fs::read_to_string(baseline_path(kver)).expect("read baseline");
        // Once real logs are committed (post-CI), this assertion is
        // expected to start failing — at which point the test should
        // be updated to assert structural verifier-log shape instead.
        assert!(
            body.contains("HARNESS-CAPTURED-PENDING-CI-RERUN"),
            "baseline {kver} no longer carries the pending marker — \
             either (a) CI has refreshed it (good — update this test), \
             or (b) the marker was stripped accidentally (bad)",
        );
    }
}

#[test]
fn script_help_exits_with_usage_code() {
    let status = Command::new("bash")
        .arg(script_path())
        .arg("--help")
        .status()
        .expect("invoke verify-xdp.sh --help");
    // The script uses exit 64 (EX_USAGE) for usage / --help — same as
    // BSD `sysexits.h`. This is the contract OPS-09's doc-lint relies on.
    assert_eq!(status.code(), Some(64), "--help must exit 64 (EX_USAGE)");
}

#[test]
fn script_rejects_unknown_kernel() {
    let status = Command::new("bash")
        .arg(script_path())
        .args(["--kernel", "9.99"])
        .status()
        .expect("invoke verify-xdp.sh --kernel 9.99");
    assert_eq!(status.code(), Some(64), "unknown kernel must exit 64");
}

#[test]
fn script_rejects_floating_image_without_override() {
    // The script must refuse to run with the floating lvh-images tag
    // unless EG_ALLOW_FLOATING_IMAGE=1 is set. Reproducibility.
    let output = Command::new("bash")
        .arg(script_path())
        .args(["--kernel", "6.1"])
        .env_remove("EG_ALLOW_FLOATING_IMAGE")
        .output()
        .expect("invoke verify-xdp.sh");
    // Exit 3 = environment problem (no pinned digest, no override).
    assert_eq!(output.status.code(), Some(3), "should fail with exit 3");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no pinned digest"),
        "stderr must explain the failure, got: {stderr}",
    );
}
