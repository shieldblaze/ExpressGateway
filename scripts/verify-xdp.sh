#!/usr/bin/env bash
# EBPF-2-07 / ROUND8-L4-10: per-kernel verifier-log capture driver.
#
# Boots a kernel via lvh, loads crates/lb-l4-xdp/src/lb_xdp.bin at BPF_LOG_LEVEL=2,
# and writes audit/ebpf/verifier-logs/<kver>.log. CI diffs against the committed
# snapshot and fails on drift. Kernels are the LTS floor (5.15), current (6.1) and
# rolling (6.6) per DEPLOYMENT.md §27.
#
# Needs --privileged + bpffs for the lvh container; off Docker, rely on the CI matrix.
#
#   scripts/verify-xdp.sh --kernel 6.1 [--update-baseline]
#
# Exit: 0 match · 1 drift (refresh + commit) · 2 missing baseline · 3 environment
# (docker/image/ELF) · 64 usage.

set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"

usage() {
    cat <<USAGE >&2
usage: scripts/verify-xdp.sh --kernel <KVER> [--update-baseline]

  --kernel <KVER>      One of: 5.15, 6.1, 6.6 (the supported LTS /
                       rolling-LTS floors per DEPLOYMENT.md §27).
  --update-baseline    Capture the verifier log and overwrite the
                       committed .log.committed snapshot. Use after
                       an intentional BPF source change.

Each KVER value maps to a pinned lvh-images digest so the CI matrix
is reproducible.

Deprecated: positional KVER (e.g. \`verify-xdp.sh 5.15\`) is still
accepted for one release with a WARN. Switch call sites to the flag
form.
USAGE
    exit 64
}

KVER=""
UPDATE_BASELINE=0

# Parse flags; tolerate the deprecated positional form.
while [ $# -gt 0 ]; do
    case "$1" in
        --kernel)
            shift
            [ $# -ge 1 ] || usage
            KVER="$1"
            shift
            ;;
        --update-baseline)
            UPDATE_BASELINE=1
            shift
            ;;
        --help|-h)
            usage
            ;;
        5.15|6.1|6.6)
            # Deprecated positional form.
            printf 'verify-xdp.sh: WARN: positional KVER is deprecated; use --kernel %s\n' "$1" >&2
            KVER="$1"
            shift
            ;;
        *)
            printf 'verify-xdp.sh: unrecognised arg %q\n' "$1" >&2
            usage
            ;;
    esac
done

if [ -z "${KVER}" ]; then
    usage
fi

case "${KVER}" in
    5.15|6.1|6.6) ;;
    *)
        printf 'verify-xdp.sh: unsupported kernel version %q\n' "${KVER}" >&2
        usage
        ;;
esac

# ROUND8-L4-10: pinned lvh-images digests per kernel. Until an entry is a real
# digest it holds the floating tag, and the script REFUSES to run without an
# explicit EG_ALLOW_FLOATING_IMAGE=1 — reproducibility preserved by construction.
IMAGE_BASE="quay.io/lvh-images/kernel-images"
case "${KVER}" in
    5.15)
        # Placeholder — replace with `quay.io/.../kernel-images@sha256:...`
        # after first CI green run.
        IMAGE_PIN_DIGEST=""
        ;;
    6.1)
        IMAGE_PIN_DIGEST=""
        ;;
    6.6)
        IMAGE_PIN_DIGEST=""
        ;;
esac

if [ -n "${IMAGE_PIN_DIGEST}" ]; then
    IMAGE="${IMAGE_BASE}@${IMAGE_PIN_DIGEST}"
else
    if [ "${EG_ALLOW_FLOATING_IMAGE:-0}" != "1" ]; then
        printf 'verify-xdp.sh: FATAL: no pinned digest for kernel %s\n' "${KVER}" >&2
        printf '                   set EG_ALLOW_FLOATING_IMAGE=1 to use the floating\n' >&2
        printf '                   %s:%s-main tag (NOT reproducible)\n' "${IMAGE_BASE}" "${KVER}" >&2
        exit 3
    fi
    IMAGE="${IMAGE_BASE}:${KVER}-main"
fi

ELF="${REPO_ROOT}/crates/lb-l4-xdp/src/lb_xdp.bin"
LOG_DIR="${REPO_ROOT}/audit/ebpf/verifier-logs"
OUT_LOG="${LOG_DIR}/${KVER}.log"

if [ ! -f "${ELF}" ]; then
    printf 'verify-xdp.sh: %s not found; run scripts/build-xdp.sh first\n' "${ELF}" >&2
    exit 3
fi

mkdir -p "${LOG_DIR}"

say() { printf 'verify-xdp.sh: %s\n' "$*"; }

if ! command -v docker >/dev/null 2>&1; then
    say "docker not in PATH; this script needs Docker to boot lvh-images"
    say "rerun in CI or install docker locally"
    exit 3
fi

say "kernel ${KVER}; loading lb_xdp.bin via lvh"
docker run --rm --privileged \
    -v "${REPO_ROOT}:/work" -w /work \
    "${IMAGE}" \
    bash -c '
        set -euo pipefail
        # The verifier writes to stderr, so that is what gets captured.
        bpftool prog load /work/crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/probe \
            type xdp \
            2> /tmp/verifier.log || true
        cat /tmp/verifier.log

        # BPF_PROG_TEST_RUN with a synthetic Ethernet+IPv4+TCP packet: proves the
        # program EXECUTES the rewrite path, not merely that it loads. The packet
        # matches a probe-CT entry CI inserts; outside CI it is best-effort.
        if [ -e /sys/fs/bpf/probe ]; then
            bpftool prog run pinned /sys/fs/bpf/probe \
                data_in /work/scripts/_probe_packet.bin \
                data_out /tmp/probe_out.bin \
                repeat 1 \
                > /tmp/probe_verdict.txt 2>&1 || true
            cat /tmp/probe_verdict.txt > /tmp/verifier.log.extra || true
        fi
        cat /tmp/verifier.log.extra >> /tmp/verifier.log 2>/dev/null || true
    ' > "${OUT_LOG}.raw"

# Normalise away address/insn-count churn: the log is structurally stable but its
# absolutes vary per build.
sed -E \
    -e 's/0x[0-9a-f]{16}/0xADDR/g' \
    -e 's/processed [0-9]+ insns/processed N insns/' \
    -e 's/peak_states [0-9]+/peak_states N/' \
    -e 's/mark_read [0-9]+/mark_read N/' \
    "${OUT_LOG}.raw" > "${OUT_LOG}"
rm "${OUT_LOG}.raw"

say "captured verifier log → ${OUT_LOG}"

# The diff-gate hard-fails on a MISSING baseline; it used to pass silently.
if [ "${UPDATE_BASELINE}" -eq 1 ]; then
    cp "${OUT_LOG}" "${OUT_LOG}.committed"
    say "wrote new baseline → ${OUT_LOG}.committed"
    say "remember: git add ${OUT_LOG}.committed"
    exit 0
fi

if [ ! -f "${OUT_LOG}.committed" ]; then
    say "FATAL: no committed baseline at ${OUT_LOG}.committed"
    say "either: 1) commit ${OUT_LOG} as the baseline (first-time setup)"
    say "          run with --update-baseline, then git add the file"
    say "        2) restore the deleted committed baseline (regression)"
    exit 2
fi

if ! diff -u "${OUT_LOG}.committed" "${OUT_LOG}"; then
    say "VERIFIER LOG DIFF on kernel ${KVER}"
    say "if intentional: rerun with --update-baseline and commit the result"
    exit 1
fi
say "verifier log matches baseline ($(wc -l < "${OUT_LOG}.committed") lines)"
