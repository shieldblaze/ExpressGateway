//! F-ESC-1 — REAL kernel-7.0 eBPF verifier baseline capture.
//!
//! Loads the shipped `lb_xdp.bin` via the PROVEN aya path (the same
//! `XdpLoader::load_from_bytes_pinned` + `kernel_load` the D-1 test
//! uses — a genuine `BPF_PROG_LOAD` on the running kernel) and then
//! reads the loaded program's REAL kernel verifier-derived facts via
//! aya `ProgramInfo` (`verified_instruction_count` = kernel
//! `verified_insns`, `size_translated` = xlated bytes, `size_jitted` =
//! jited bytes, `tag`, `name`, prog id). These are genuine kernel
//! verifier outputs, NOT a placeholder.
//!
//! It then ALSO runs `bpftool prog show id <id> --json` on the
//! aya-loaded prog (bpftool can INSPECT an already-loaded prog even
//! though libbpf cannot LOAD this legacy-map ELF — auditor-3 tooling
//! note) for the authoritative kernel verifier STATS line, and writes
//! a structured real baseline to
//! `audit/ebpf/verifier-logs/7.0.log.committed` (uname, counters,
//! GPL license assertion, capture method, timestamp).
//!
//! NO attach is performed (no MTU/channel disruption): the load alone
//! is what the kernel verifier runs, so the load is sufficient for the
//! verifier baseline.
//!
//! Privileged + `#[ignore]`d (needs CAP_BPF + bpffs). Run via:
//!   sudo -E env "PATH=$PATH" cargo test -p lb-l4-xdp \
//!     --test round8_verifier_baseline_70 -- --ignored --nocapture

use lb_l4_xdp::loader::XdpLoader;
use lb_l4_xdp::LB_XDP_ELF;
use std::path::Path;
use std::process::Command;

const PROG: &str = "lb_xdp";
const BPFFS: &str = "/sys/fs/bpf";
const OUT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../audit/ebpf/verifier-logs/7.0.log.committed"
);

#[test]
#[ignore = "privileged: real BPF_PROG_LOAD on the running kernel to capture the \
            verifier baseline — run via sudo -E cargo test -p lb-l4-xdp \
            --test round8_verifier_baseline_70 -- --ignored --nocapture"]
fn capture_real_70_verifier_baseline() {
    fn cmd(bin: &str, args: &[&str]) -> String {
        Command::new(bin)
            .args(args)
            .output()
            .map(|o| {
                let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
                if !o.stderr.is_empty() {
                    s.push_str(&String::from_utf8_lossy(&o.stderr));
                }
                s.trim().to_string()
            })
            .unwrap_or_else(|e| format!("<{bin} unavailable: {e}>"))
    }

    let uname = cmd("uname", &["-a"]);
    let kver = cmd("uname", &["-r"]);

    // Real BPF_PROG_LOAD via the proven aya loader (legacy-map ELF —
    // aya is the only loader that can load it; D-1 proved this path).
    let mut loader = XdpLoader::load_from_bytes_pinned(LB_XDP_ELF, Some(Path::new(BPFFS)))
        .expect("load_from_bytes_pinned(lb_xdp.bin, /sys/fs/bpf)");
    loader
        .kernel_load(PROG)
        .expect("kernel_load(lb_xdp) — real BPF_PROG_LOAD on the running 7.0 kernel");

    // Read the loaded program's REAL kernel verifier-derived facts.
    use aya::programs::{Program, Xdp};
    let ebpf = loader.ebpf_mut();
    let prog: &mut Program = ebpf
        .program_mut(PROG)
        .expect("program_mut(lb_xdp) present after kernel_load");
    let xdp: &Xdp = (&*prog)
        .try_into()
        .expect("lb_xdp is an Xdp program");
    let info = xdp.info().expect("ProgramInfo for the loaded prog");

    let prog_id = info.id();
    let tag = info.tag();
    let name = info
        .name_as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{:?}", info.name()));
    let verified_insns = info.verified_instruction_count();
    let xlated = info.size_translated();
    let jited = info.size_jitted();

    // Authoritative kernel STATS via bpftool on the aya-loaded prog.
    let bpftool_json = cmd(
        "bpftool",
        &["prog", "show", "id", &prog_id.to_string(), "--json"],
    );
    let bpftool_plain = cmd("bpftool", &["prog", "show", "id", &prog_id.to_string()]);

    // GPL license assertion: lb_xdp must declare GPL or the kernel
    // would reject GPL-only helpers — proven by a successful load.
    let gpl_assert = "GPL license: ASSERTED (BPF_PROG_LOAD succeeded; the kernel rejects \
                      GPL-only helpers used by lb_xdp under a non-GPL license, so a \
                      successful real load is proof of the GPL declaration).";

    let ts = cmd("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]);

    let body = format!(
        "# REAL eBPF verifier baseline — kernel {kver}\n\
         #\n\
         # F-ESC-1 (foundation audit). This is a REAL capture, NOT the\n\
         # HARNESS-CAPTURED-PENDING-CI-RERUN placeholder. Method: the\n\
         # shipped crates/lb-l4-xdp/src/lb_xdp.bin was loaded via the\n\
         # proven aya path (XdpLoader::load_from_bytes_pinned +\n\
         # kernel_load -> a genuine BPF_PROG_LOAD on the running\n\
         # kernel; libbpf/bpftool CANNOT load this legacy-map ELF, so\n\
         # aya is the only real loader — D-1 proved this path). The\n\
         # counters below are the kernel verifier's own outputs read\n\
         # back via aya ProgramInfo + bpftool prog show on the loaded\n\
         # prog id. No attach was performed (the load is what runs the\n\
         # verifier).\n\
         #\n\
         capture_method   = aya XdpLoader::kernel_load (real BPF_PROG_LOAD) + aya ProgramInfo + bpftool prog show id <id>\n\
         capture_timestamp = {ts}\n\
         uname            = {uname}\n\
         kernel_release   = {kver}\n\
         elf              = crates/lb-l4-xdp/src/lb_xdp.bin\n\
         prog_name        = {name}\n\
         prog_id          = {prog_id}\n\
         prog_tag         = {tag:#018x}\n\
         verified_insns   = {verified_insns:?}   # kernel verified_insns (None if pre-5.16; 7.0 reports it)\n\
         size_translated  = {xlated:?}   # xlated bytes\n\
         size_jitted      = {jited}   # jited bytes\n\
         {gpl_assert}\n\
         \n\
         ## bpftool prog show id {prog_id} (plain)\n\
         {bpftool_plain}\n\
         \n\
         ## bpftool prog show id {prog_id} --json\n\
         {bpftool_json}\n",
    );

    std::fs::write(OUT, &body).expect("write 7.0.log.committed");
    eprintln!("F-ESC-1: wrote real 7.0 verifier baseline to {OUT}\n{body}");

    // Sanity: the capture must carry real kernel counters (the
    // explanatory header intentionally NAMES the old placeholder
    // marker, so we assert on the data fields, not prose).
    assert!(prog_id > 0, "loaded prog must have a real kernel id");
    assert!(tag != 0, "prog tag must be a real kernel-computed tag");
    assert!(
        verified_insns.is_some_and(|n| n > 0),
        "kernel 7.0 must report verified_insns > 0 (got {verified_insns:?})"
    );
}
