//! Read a process's resident-memory / fd / thread footprint from
//! `/proc/<pid>/`.
//!
//! This is the OS-level half of the soak's stability signal (the other half
//! is the product's own `/metrics` state gauges; see [`crate::metrics`]). The
//! gateway runs as a separate child process, so its RSS/fd/threads are cleanly
//! isolated from the in-process load generator's — exactly why the soak
//! launches the real binary rather than driving an in-process gateway.
//!
//! Verdict-critical: a wrong RSS/fd read produces a wrong leak verdict, so the
//! parsers below are unit-tested against synthetic `/proc/<pid>/status` text
//! and against the test process's own real `/proc` entry.

use std::path::Path;

/// A single point-in-time footprint of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcFootprint {
    /// Resident set size in kibibytes (`VmRSS`). The primary leak signal.
    pub rss_kb: u64,
    /// Peak resident set size in kibibytes (`VmHWM`). Should plateau.
    pub vmhwm_kb: u64,
    /// Live thread count (`Threads`). The runtime worker pool is fixed-size,
    /// so this is the runtime-thread leak signal.
    pub threads: u64,
    /// Open file-descriptor count (entries under `/proc/<pid>/fd`). The fd
    /// leak / socket-pinning signal.
    pub fds: u64,
}

/// Parse the `VmRSS` / `VmHWM` / `Threads` lines out of `/proc/<pid>/status`
/// text. Lines look like `VmRSS:\t   12345 kB`. Missing fields stay 0.
///
/// Separated from the I/O so it can be unit-tested on a fixed fixture.
#[must_use]
pub fn parse_status(status: &str) -> ProcFootprint {
    let mut fp = ProcFootprint::default();
    for line in status.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // The numeric value is the first whitespace-separated token of `rest`
        // (the trailing `kB` unit is ignored).
        let val = rest
            .split_whitespace()
            .next()
            .and_then(|t| t.parse::<u64>().ok());
        match key.trim() {
            "VmRSS" => fp.rss_kb = val.unwrap_or(0),
            "VmHWM" => fp.vmhwm_kb = val.unwrap_or(0),
            "Threads" => fp.threads = val.unwrap_or(0),
            _ => {}
        }
    }
    fp
}

/// Count the entries under `/proc/<pid>/fd` (each is one open fd). Returns 0 if
/// the directory cannot be read (e.g. the process already exited).
#[must_use]
pub fn count_fds(proc_dir: &Path) -> u64 {
    let fd_dir = proc_dir.join("fd");
    match std::fs::read_dir(&fd_dir) {
        Ok(entries) => entries.count() as u64,
        Err(_) => 0,
    }
}

/// Sample a live pid's footprint by reading `/proc/<pid>/status` and counting
/// `/proc/<pid>/fd`. Returns `None` only if `/proc/<pid>/status` is unreadable
/// (the process is gone) — fd count alone is not fatal (it falls back to 0).
#[must_use]
pub fn sample_pid(pid: u32) -> Option<ProcFootprint> {
    let proc_dir = Path::new("/proc").join(pid.to_string());
    let status = std::fs::read_to_string(proc_dir.join("status")).ok()?;
    let mut fp = parse_status(&status);
    fp.fds = count_fds(&proc_dir);
    Some(fp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_fields() {
        // Real-shaped /proc/<pid>/status excerpt (tabs + `kB` unit).
        let status = "Name:\texpressgateway\n\
                      State:\tS (sleeping)\n\
                      VmPeak:\t  900000 kB\n\
                      VmRSS:\t   45678 kB\n\
                      VmHWM:\t   52000 kB\n\
                      Threads:\t9\n\
                      voluntary_ctxt_switches:\t100\n";
        let fp = parse_status(status);
        assert_eq!(fp.rss_kb, 45678, "VmRSS must parse");
        assert_eq!(fp.vmhwm_kb, 52000, "VmHWM must parse");
        assert_eq!(fp.threads, 9, "Threads must parse");
    }

    #[test]
    fn missing_fields_stay_zero() {
        let fp = parse_status("Name:\tfoo\nState:\tR (running)\n");
        assert_eq!(fp, ProcFootprint::default(), "absent fields default to 0");
    }

    #[test]
    fn malformed_lines_are_skipped() {
        // No colon, empty value, non-numeric value — none must panic.
        let fp = parse_status("garbage line\nVmRSS:\t kB\nThreads:\tNaN\nVmHWM:\t7 kB\n");
        assert_eq!(fp.rss_kb, 0);
        assert_eq!(fp.threads, 0);
        assert_eq!(fp.vmhwm_kb, 7);
    }

    #[test]
    fn samples_own_process() {
        // The test process is alive, so its own footprint must be readable
        // with a non-zero RSS and at least one thread + fd. This exercises
        // the real /proc I/O path (Linux-only; the soak is Linux-only).
        let pid = std::process::id();
        let fp = sample_pid(pid).expect("own /proc/<pid>/status must be readable");
        assert!(fp.rss_kb > 0, "own RSS must be > 0, got {}", fp.rss_kb);
        assert!(fp.threads >= 1, "own thread count must be >= 1");
        assert!(fp.fds >= 1, "own fd count must be >= 1 (stdin at minimum)");
    }

    #[test]
    fn gone_pid_returns_none() {
        // PID 0 has no /proc/0/status — sample must return None, not panic.
        assert!(sample_pid(0).is_none(), "pid 0 must yield None");
    }
}
