#!/usr/bin/env bash
# docker-smoke.sh — prove a RUNNING expressgateway container serves real
# L7 traffic, not merely that `docker build` succeeded.
#
# What it does:
#   1. creates a user-defined docker network,
#   2. starts a tiny backend (hashicorp/http-echo) on it that returns a
#      known body + 200,
#   3. starts the gateway container `expressgateway:smoke` on the same
#      network with docker/smoke/gateway.toml mounted as the config and
#      the plaintext listener port published to the host,
#   4. waits for the gateway listener to accept connections,
#   5. sends a REAL HTTP/1.1 request THROUGH the gateway to the published
#      port and asserts the response is the backend's (200 + expected body).
#
# This is the bar D5-image-scan does NOT cover: D5 builds + Trivy-scans the
# image; this proves it BOOTS and SERVES.
#
# Assumes the image `expressgateway:smoke` is ALREADY built — this script
# does NOT build it (the CI job / lead builds it first).
#
# Rootless-docker friendly: no sudo, all traffic flows container->container
# over the user-defined network (so no host.docker.internal dependency);
# only the final assertion curls the published host port.
set -euo pipefail

# ── tunables ─────────────────────────────────────────────────────────────
IMAGE="${IMAGE:-expressgateway:smoke}"
BACKEND_IMAGE="${BACKEND_IMAGE:-hashicorp/http-echo:latest}"
NET="${NET:-eg-smoke-net}"
GW_NAME="${GW_NAME:-eg-smoke-gw}"
BE_NAME="${BE_NAME:-eg-smoke-backend}"
# The gateway resolves the backend by the hostname in docker/smoke/gateway.toml
# (`backend:8080`) at boot — so the backend container needs that exact DNS name
# on the user-defined network. `--name` is eg-smoke-backend (idempotent cleanup
# handle); `--network-alias backend` is what the config resolves.
BE_ALIAS="${BE_ALIAS:-backend}"
EXPECTED_BODY="${EXPECTED_BODY:-eg-smoke-ok}"
# Host port the published gateway listener is reached on. The in-container
# listener is 8080 (docker/smoke/gateway.toml). Use a high host port to
# avoid privileged-port issues under rootless docker.
HOST_PORT="${HOST_PORT:-18080}"
GW_PORT=8080         # in-container plaintext listener (gateway.toml)
BE_PORT=8080         # in-container backend port (http-echo default)
READY_TIMEOUT="${READY_TIMEOUT:-40}"   # seconds to wait for the gateway

# Resolve the repo root so the bind-mount path is absolute regardless of
# the caller's cwd (docker -v requires an absolute source path).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SMOKE_CONFIG="${REPO_ROOT}/docker/smoke/gateway.toml"

log()  { printf '[docker-smoke] %s\n' "$*" >&2; }
fail() { printf '[docker-smoke] FAIL: %s\n' "$*" >&2; exit 1; }

# ── teardown (runs on every exit, success or failure) ────────────────────
cleanup() {
  local rc=$?
  log "tearing down…"
  docker rm -f "${GW_NAME}" >/dev/null 2>&1 || true
  docker rm -f "${BE_NAME}" >/dev/null 2>&1 || true
  docker network rm "${NET}" >/dev/null 2>&1 || true
  if [ "${rc}" -eq 0 ]; then
    log "PASS — running container served a real request through the gateway"
  else
    log "FAILED (exit ${rc})"
  fi
  return "${rc}"
}
trap cleanup EXIT

# ── preflight ────────────────────────────────────────────────────────────
command -v docker >/dev/null 2>&1 || fail "docker not found on PATH"
command -v curl   >/dev/null 2>&1 || fail "curl not found on PATH"
[ -f "${SMOKE_CONFIG}" ] || fail "smoke config not found: ${SMOKE_CONFIG}"
docker image inspect "${IMAGE}" >/dev/null 2>&1 \
  || fail "image ${IMAGE} not present — build it first (the CI job does this)"

# Idempotency: clear any leftovers from a previous aborted run.
docker rm -f "${GW_NAME}" "${BE_NAME}" >/dev/null 2>&1 || true
docker network rm "${NET}" >/dev/null 2>&1 || true

# ── 1. network ───────────────────────────────────────────────────────────
log "creating network ${NET}"
docker network create "${NET}" >/dev/null

# ── 2. backend ───────────────────────────────────────────────────────────
# http-echo serves the same body on every path/method with status 200.
log "starting backend (${BACKEND_IMAGE}) -> body '${EXPECTED_BODY}'"
docker run -d --name "${BE_NAME}" --network "${NET}" \
  --network-alias "${BE_ALIAS}" \
  "${BACKEND_IMAGE}" \
  -listen=":${BE_PORT}" -text="${EXPECTED_BODY}" >/dev/null \
  || fail "backend failed to start"

# ── 3. gateway ───────────────────────────────────────────────────────────
# Mount the smoke config over the image's default config path (argv[1]).
# Publish the in-container listener to HOST_PORT so the host curl reaches it.
log "starting gateway (${IMAGE}) — config mounted, :${GW_PORT} published to :${HOST_PORT}"
docker run -d --name "${GW_NAME}" --network "${NET}" \
  -p "127.0.0.1:${HOST_PORT}:${GW_PORT}" \
  -v "${SMOKE_CONFIG}:/etc/expressgateway/config.toml:ro" \
  "${IMAGE}" >/dev/null \
  || fail "gateway failed to start"

# ── 4. wait for the gateway listener ─────────────────────────────────────
log "waiting up to ${READY_TIMEOUT}s for the gateway to serve…"
ready=0
for _ in $(seq 1 "${READY_TIMEOUT}"); do
  # The gateway must still be running…
  if [ "$(docker inspect -f '{{.State.Running}}' "${GW_NAME}" 2>/dev/null)" != "true" ]; then
    log "gateway container exited early — logs:"
    docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
    fail "gateway container is not running"
  fi
  # …and the published port must answer HTTP. -s/-o keeps output quiet; we
  # only need a non-empty HTTP status line to know the listener is up.
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 \
            "http://127.0.0.1:${HOST_PORT}/" 2>/dev/null || true)"
  if [ -n "${code}" ] && [ "${code}" != "000" ]; then
    ready=1
    break
  fi
  sleep 1
done

if [ "${ready}" -ne 1 ]; then
  log "gateway did not come up — gateway logs:"
  docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
  log "backend logs:"
  docker logs "${BE_NAME}" 2>&1 | sed 's/^/  be| /' >&2 || true
  fail "gateway listener never became reachable on :${HOST_PORT}"
fi
log "gateway is accepting connections"

# ── 5. the actual proof: a real request THROUGH the gateway ──────────────
log "sending a real HTTP/1.1 request through the gateway -> backend"
resp="$(curl -s --max-time 5 -w $'\n%{http_code}' \
          "http://127.0.0.1:${HOST_PORT}/smoke" 2>/dev/null || true)"
body="$(printf '%s' "${resp}" | sed '$d')"   # everything but the last line
status="$(printf '%s' "${resp}" | tail -n1)" # last line = http_code

log "gateway returned status=${status} body='${body}'"

if [ "${status}" != "200" ]; then
  log "unexpected status — gateway logs:"
  docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
  fail "expected HTTP 200 from the backend through the gateway, got '${status}'"
fi

# http-echo appends a trailing newline to -text; match as a substring so we
# are robust to whitespace.
case "${body}" in
  *"${EXPECTED_BODY}"*) : ;;
  *)
    log "body mismatch — gateway logs:"
    docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
    fail "response body did not contain backend marker '${EXPECTED_BODY}'"
    ;;
esac

log "verified: 200 + backend body proxied through the running container"
# trap cleanup runs here and prints PASS.
exit 0
