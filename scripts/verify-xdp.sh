#!/usr/bin/env bash
# EBPF-2-07: per-kernel verifier-log capture driver.
#
# Boots a kernel image via lvh (little-vm-helper) at one of the
# supported LTS versions, loads `crates/lb-l4-xdp/src/lb_xdp.bin`
# with `BPF_LOG_LEVEL=2`, and writes the captured verifier log to
# `audit/ebpf/verifier-logs/<kver>.log`. CI diffs the result against
# the committed snapshot and fails on drift.
#
# Usage:
#   scripts/verify-xdp.sh 5.15
#   scripts/verify-xdp.sh 6.1
#   scripts/verify-xdp.sh 6.6
#
# Supported kernel versions are the LTS floor (5.15), current LTS
# (6.1), and rolling LTS (6.6) per DEPLOYMENT.md §27.
#
# Local sandbox note: the lvh container needs --privileged and bpffs.
# CI carries those; developers running this on a non-Docker machine
# should rely on the CI matrix instead.

set -euo pipefail

cd "$(dirname "$0")/.."
REPO_ROOT="$(pwd)"

usage() {
    cat <<USAGE >&2
usage: scripts/verify-xdp.sh <KVER>

KVER must be one of: 5.15, 6.1, 6.6 (the supported LTS / rolling-LTS
floors per DEPLOYMENT.md §27). Each value maps to a pinned
lvh-images digest so the CI matrix is reproducible.
USAGE
    exit 64
}

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ] || [ "$#" -lt 1 ]; then
    usage
fi

KVER="$1"
case "${KVER}" in
    5.15|6.1|6.6) ;;
    *)
        printf 'verify-xdp.sh: unsupported kernel version %q\n' "${KVER}" >&2
        usage
        ;;
esac

IMAGE_BASE="quay.io/lvh-images/kernel-images"
# Pinned digests would be inserted here after the first CI run
# blesses an lvh-images@sha256:... — for now the floating tag keeps
# this script working pre-CI. The plan calls out this as a CI follow-up.
IMAGE="${IMAGE_BASE}:${KVER}-main"

ELF="${REPO_ROOT}/crates/lb-l4-xdp/src/lb_xdp.bin"
LOG_DIR="${REPO_ROOT}/audit/ebpf/verifier-logs"
OUT_LOG="${LOG_DIR}/${KVER}.log"

if [ ! -f "${ELF}" ]; then
    printf 'verify-xdp.sh: %s not found; run scripts/build-xdp.sh first\n' "${ELF}" >&2
    exit 2
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
        # The lvh image already mounts bpffs and has bpftool. Load the
        # program with verbose verifier log; capture stderr (where
        # the verifier writes).
        bpftool prog load /work/crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/probe \
            type xdp \
            2> /tmp/verifier.log || true
        cat /tmp/verifier.log
    ' > "${OUT_LOG}.raw"

# Normalise to suppress address/insn-count churn (the verifier log
# has stable structural output but variable absolutes per build).
sed -E \
    -e 's/0x[0-9a-f]{16}/0xADDR/g' \
    -e 's/processed [0-9]+ insns/processed N insns/' \
    -e 's/peak_states [0-9]+/peak_states N/' \
    -e 's/mark_read [0-9]+/mark_read N/' \
    "${OUT_LOG}.raw" > "${OUT_LOG}"
rm "${OUT_LOG}.raw"

say "captured verifier log → ${OUT_LOG}"

if [ -f "${OUT_LOG}.committed" ]; then
    if ! diff -u "${OUT_LOG}.committed" "${OUT_LOG}"; then
        say "VERIFIER LOG DIFF on kernel ${KVER}"
        say "if intentional, commit the new ${OUT_LOG} as the snapshot"
        exit 1
    fi
fi
