#!/usr/bin/env bash
# release-soak.sh — the RELEASE soak gate CONTROLLER (on-demand EC2).
#
# Provisions a dedicated soak EC2, runs the full 12-scenario lb-soak on it for a
# release-grade duration, fetches the PASS/FAIL verdict, and ALWAYS tears the
# instance down (a trap, even on failure/timeout/interrupt). Exits non-zero iff
# any scenario was not BOUNDED (or panicked) — gating the RELEASE.
#
# This gates RELEASE, not every PR (hosted CI cannot soak — see DEV-SETUP.md).
# It is invoked manually / by the release workflow (release.yml, workflow_dispatch).
#
# MODEL (owner-confirmed S40): on-demand provision -> run -> report -> TEARDOWN.
# Auth: in CI, GitHub OIDC assumes a scoped IAM role (no long-lived keys) before
# this runs; locally, configure AWS creds in the environment first.
#
#   controller (here): run-instances (user-data bootstraps the box) -> poll S3
#                      for the DONE sentinel -> download summary -> terminate.
#   on the box (user-data -> scripts/soak/release-soak-onbox.sh): clone@REF ->
#                      build -> 12-scenario soak -> verdict -> upload to S3 ->
#                      self-terminate (belt-and-suspenders with the trap).
#
# Validate without spending money:  ./scripts/release-soak.sh --dry-run
#   prints every AWS command (stubbed) and the rendered user-data, runs nothing.
#
# ---- required configuration (env) ----------------------------------------
#   SOAK_REGION                AWS region                (default: $AWS_REGION)
#   SOAK_AMI                   Ubuntu 24.04 x86_64 AMI id          (required)
#   SOAK_SUBNET_ID             subnet for the instance             (required)
#   SOAK_SECURITY_GROUP_ID     SG (egress to GitHub + S3 is enough)(required)
#   SOAK_IAM_INSTANCE_PROFILE  instance profile: PutObject to the  (required)
#                              result bucket + ec2:TerminateInstances(self)
#   SOAK_S3_BUCKET             result bucket                       (required)
# ---- optional configuration ----------------------------------------------
#   SOAK_INSTANCE_TYPE         default c6a.2xlarge (8 vCPU + ENA — representative)
#   SOAK_S3_PREFIX             default release-soak/<ref>-<utcstamp>
#   SOAK_GIT_REPO              default the current origin URL
#   SOAK_GIT_REF               default the current git ref (tag/sha)
#   SOAK_DURATION_SECS         default 14400 (4h)
#   SOAK_SAMPLE_SECS           default 30
#   SOAK_MAX_WAIT_SECS         controller poll budget; default DURATION + 3600
#   SOAK_KEY_NAME              optional EC2 keypair (debug SSH; not required)
set -uo pipefail

DRY_RUN=0
[ "${1:-}" = "--dry-run" ] && { DRY_RUN=1; shift; }

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ---- resolve config -------------------------------------------------------
SOAK_REGION="${SOAK_REGION:-${AWS_REGION:-}}"
SOAK_INSTANCE_TYPE="${SOAK_INSTANCE_TYPE:-c6a.2xlarge}"
SOAK_DURATION_SECS="${SOAK_DURATION_SECS:-14400}"
SOAK_SAMPLE_SECS="${SOAK_SAMPLE_SECS:-30}"
SOAK_MAX_WAIT_SECS="${SOAK_MAX_WAIT_SECS:-$((SOAK_DURATION_SECS + 3600))}"
SOAK_GIT_REPO="${SOAK_GIT_REPO:-$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null || echo UNKNOWN)}"
SOAK_GIT_REF="${SOAK_GIT_REF:-$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || echo HEAD)}"
# A filesystem/S3-safe stamp without Date.now()-style fragility: derive from the ref.
SOAK_S3_PREFIX="${SOAK_S3_PREFIX:-release-soak/${SOAK_GIT_REF}}"

die()  { echo "::error::$*" >&2; exit 2; }
log()  { echo "[release-soak $(date -u +%H:%M:%S)] $*"; }

# aws wrapper: in --dry-run, print and skip; else execute.
aws_do() {
  if [ "$DRY_RUN" -eq 1 ]; then echo "    DRY-RUN aws $*"; return 0; fi
  aws "$@"
}

# ---- preflight ------------------------------------------------------------
[ -n "$SOAK_REGION" ] || die "SOAK_REGION (or AWS_REGION) is required"
if [ "$DRY_RUN" -eq 0 ]; then
  command -v aws >/dev/null 2>&1 || die "aws CLI not found"
  for v in SOAK_AMI SOAK_SUBNET_ID SOAK_SECURITY_GROUP_ID SOAK_IAM_INSTANCE_PROFILE SOAK_S3_BUCKET; do
    [ -n "${!v:-}" ] || die "$v is required"
  done
fi
S3_DEST="s3://${SOAK_S3_BUCKET:-DRYRUN-BUCKET}/${SOAK_S3_PREFIX}"

log "region=$SOAK_REGION type=$SOAK_INSTANCE_TYPE dur=${SOAK_DURATION_SECS}s ref=$SOAK_GIT_REF"
log "result handoff: $S3_DEST"

# ---- user-data (bootstraps + soaks + uploads + self-terminates) -----------
USER_DATA=$(cat <<EOF
#!/usr/bin/env bash
set -uo pipefail
exec > /var/log/release-soak-bootstrap.log 2>&1
echo "[bootstrap \$(date -u)] release soak box up"
export DEBIAN_FRONTEND=noninteractive
apt-get update -y
apt-get install -y git curl build-essential cmake clang libclang-dev llvm pkg-config awscli
# Rust at the pinned MSRV (rust-toolchain.toml is honored by rustup on build).
curl -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.88
export PATH="\$HOME/.cargo/bin:/root/.cargo/bin:\$PATH"
cd /opt
git clone "$SOAK_GIT_REPO" eg && cd eg && git checkout "$SOAK_GIT_REF"
export CARGO_TARGET_DIR=/opt/eg-target
bash scripts/soak/release-soak-onbox.sh "$SOAK_DURATION_SECS" "$SOAK_SAMPLE_SECS" "$S3_DEST"
RC=\$?
echo "[bootstrap \$(date -u)] onbox rc=\$RC — self-terminating"
TOKEN=\$(curl -s -X PUT "http://169.254.169.254/latest/api/token" -H "X-aws-ec2-metadata-token-ttl-seconds: 60")
IID=\$(curl -s -H "X-aws-ec2-metadata-token: \$TOKEN" http://169.254.169.254/latest/meta-data/instance-id)
aws ec2 terminate-instances --region "$SOAK_REGION" --instance-ids "\$IID" || true
EOF
)

if [ "$DRY_RUN" -eq 1 ]; then
  echo "---- rendered user-data ----"; echo "$USER_DATA"; echo "---- end user-data ----"
fi

# ---- provision ------------------------------------------------------------
INSTANCE_ID=""
teardown() {
  # The trap: ALWAYS terminate (idempotent with the box's self-terminate).
  if [ -n "$INSTANCE_ID" ]; then
    log "teardown: terminating $INSTANCE_ID"
    aws_do ec2 terminate-instances --region "$SOAK_REGION" --instance-ids "$INSTANCE_ID" >/dev/null 2>&1 || true
  fi
}
trap teardown EXIT INT TERM

log "provisioning soak instance…"
RUN_ARGS=(ec2 run-instances --region "$SOAK_REGION"
  --image-id "${SOAK_AMI:-DRYRUN-AMI}" --instance-type "$SOAK_INSTANCE_TYPE"
  --subnet-id "${SOAK_SUBNET_ID:-DRYRUN-SUBNET}"
  --security-group-ids "${SOAK_SECURITY_GROUP_ID:-DRYRUN-SG}"
  --iam-instance-profile "Name=${SOAK_IAM_INSTANCE_PROFILE:-DRYRUN-PROFILE}"
  --instance-initiated-shutdown-behavior terminate
  --user-data "$USER_DATA"
  --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=eg-release-soak},{Key=eg-ref,Value=$SOAK_GIT_REF}]"
  --query 'Instances[0].InstanceId' --output text)
[ -n "${SOAK_KEY_NAME:-}" ] && RUN_ARGS+=(--key-name "$SOAK_KEY_NAME")

if [ "$DRY_RUN" -eq 1 ]; then
  echo "    DRY-RUN aws ${RUN_ARGS[*]}"
  log "DRY-RUN: would poll $S3_DEST/DONE for up to ${SOAK_MAX_WAIT_SECS}s, then teardown."
  log "DRY-RUN complete — no resources created."
  trap - EXIT INT TERM
  exit 0
fi

INSTANCE_ID=$(aws "${RUN_ARGS[@]}")
[ -n "$INSTANCE_ID" ] && [ "$INSTANCE_ID" != "None" ] || die "run-instances did not return an InstanceId"
log "provisioned $INSTANCE_ID — soaking (budget ${SOAK_MAX_WAIT_SECS}s)…"

# ---- poll S3 for the DONE sentinel ----------------------------------------
waited=0
POLL=60
VERDICT_RC=1
while [ "$waited" -lt "$SOAK_MAX_WAIT_SECS" ]; do
  if aws s3 ls "$S3_DEST/DONE" --region "$SOAK_REGION" >/dev/null 2>&1; then
    aws s3 cp "$S3_DEST/DONE" /tmp/eg-soak-DONE --region "$SOAK_REGION" >/dev/null 2>&1 || true
    aws s3 cp "$S3_DEST/release-soak-summary.txt" /tmp/eg-release-soak-summary.txt --region "$SOAK_REGION" >/dev/null 2>&1 || true
    VERDICT_RC=$(sed -nE 's/.*verdict_rc=([0-9]+).*/\1/p' /tmp/eg-soak-DONE 2>/dev/null || echo 1)
    log "DONE sentinel found (verdict_rc=$VERDICT_RC)"
    break
  fi
  sleep "$POLL"; waited=$((waited + POLL))
done

# ---- report + gate --------------------------------------------------------
if [ -f /tmp/eg-release-soak-summary.txt ]; then
  echo "================ release-soak summary ================"
  cat /tmp/eg-release-soak-summary.txt
  echo "====================================================="
else
  log "no summary retrieved (timeout after ${waited}s?)"; VERDICT_RC=1
fi

# teardown runs via the trap on exit.
if [ "${VERDICT_RC:-1}" -eq 0 ]; then
  log "RELEASE SOAK GATE: PASS"; exit 0
fi
log "RELEASE SOAK GATE: FAIL (verdict_rc=${VERDICT_RC})"; exit 1
