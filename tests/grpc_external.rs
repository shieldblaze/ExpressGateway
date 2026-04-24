//! External-tool harness (Item 3 / PROMPT.md §13).
//!
//! `grpc-health-probe` + `ghz` are the canonical production validators
//! for gRPC gateways. Neither is likely installed in our sandboxes so
//! both tests take a skip branch; the skip branch runs (not `#[ignore]`)
//! so CI always exercises the probe → skip → clean-exit path.

#![cfg(test)]

use std::process::Command;

#[tokio::test]
async fn grpc_health_probe_harness() {
    let probe = Command::new("grpc-health-probe").arg("--help").output();
    match probe {
        Ok(out) if out.status.success() => {
            // If the binary is present we would spin up the full gateway
            // + certs + invoke it. Running `grpc-health-probe` end-to-end
            // is a Phase F follow-up — for v1 we verify detection only
            // so the skip branch and the present branch both reach a
            // clean Ok().
            eprintln!(
                "grpc-health-probe detected; full harness wiring is a \
                 Phase F follow-up (see PROMPT.md §13 Scope OUT)."
            );
        }
        _ => {
            eprintln!(
                "grpc-health-probe not installed; skipping. Install via \
                 'go install github.com/grpc-ecosystem/grpc-health-probe@latest'."
            );
        }
    }
}

#[tokio::test]
async fn ghz_harness() {
    let probe = Command::new("ghz").arg("--version").output();
    match probe {
        Ok(out) if out.status.success() => {
            eprintln!(
                "ghz detected; full harness wiring is a Phase F follow-up \
                 (see PROMPT.md §13 Scope OUT)."
            );
        }
        _ => {
            eprintln!(
                "ghz not installed; skipping. Install via \
                 'go install github.com/bojand/ghz/cmd/ghz@latest'."
            );
        }
    }
}
