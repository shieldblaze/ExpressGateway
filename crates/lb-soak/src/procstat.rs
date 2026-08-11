//! `/proc/<pid>` RSS/fd/thread footprint — the OS half of the soak's stability signal.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcFootprint {
    pub rss_kb: u64,
    pub vmhwm_kb: u64,
    pub threads: u64,
    pub fds: u64,
}

#[must_use]
pub fn parse_status(status: &str) -> ProcFootprint {
    let mut fp = ProcFootprint::default();
    for line in status.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
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

#[must_use]
pub fn count_fds(proc_dir: &Path) -> u64 {
    let fd_dir = proc_dir.join("fd");
    match std::fs::read_dir(&fd_dir) {
        Ok(entries) => entries.count() as u64,
        Err(_) => 0,
    }
}

/// Sample a live pid's footprint. `None` only if `/proc/<pid>/status` is unreadable (process gone);
/// an unreadable fd dir falls back to 0.
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
        let fp = parse_status("garbage line\nVmRSS:\t kB\nThreads:\tNaN\nVmHWM:\t7 kB\n");
        assert_eq!(fp.rss_kb, 0);
        assert_eq!(fp.threads, 0);
        assert_eq!(fp.vmhwm_kb, 7);
    }

    #[test]
    fn samples_own_process() {
        let pid = std::process::id();
        let fp = sample_pid(pid).expect("own /proc/<pid>/status must be readable");
        assert!(fp.rss_kb > 0, "own RSS must be > 0, got {}", fp.rss_kb);
        assert!(fp.threads >= 1, "own thread count must be >= 1");
        assert!(fp.fds >= 1, "own fd count must be >= 1 (stdin at minimum)");
    }

    #[test]
    fn gone_pid_returns_none() {
        assert!(sample_pid(0).is_none(), "pid 0 must yield None");
    }
}
