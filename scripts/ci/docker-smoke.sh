#!/usr/bin/env bash
# Prove a RUNNING expressgateway container serves real L7 traffic — the bar
# D5-image-scan does NOT cover, since D5 only builds and Trivy-scans the image.
#
# Assumes `expressgateway:smoke` is ALREADY built; this script does not build it.
#
# Rootless-docker friendly: no sudo, traffic flows container->container over a
# user-defined network (no host.docker.internal); only the final assertion curls
# the published host port.
set -euo pipefail

IMAGE="${IMAGE:-expressgateway:smoke}"
BACKEND_IMAGE="${BACKEND_IMAGE:-hashicorp/http-echo:latest}"
NET="${NET:-eg-smoke-net}"
GW_NAME="${GW_NAME:-eg-smoke-gw}"
BE_NAME="${BE_NAME:-eg-smoke-backend}"
# gateway.toml resolves `backend:8080` at boot, so the alias — not --name, which is
# only the cleanup handle — is what must match.
BE_ALIAS="${BE_ALIAS:-backend}"
EXPECTED_BODY="${EXPECTED_BODY:-eg-smoke-ok}"
# High host port: rootless docker cannot publish privileged ones.
HOST_PORT="${HOST_PORT:-18080}"
GW_PORT=8080         # in-container plaintext listener (gateway.toml)
BE_PORT=8080         # in-container backend port (http-echo default)
READY_TIMEOUT="${READY_TIMEOUT:-40}"   # seconds to wait for the gateway

# docker -v requires an ABSOLUTE source path, whatever the caller's cwd.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SMOKE_CONFIG="${REPO_ROOT}/docker/smoke/gateway.toml"

log()  { printf '[docker-smoke] %s\n' "$*" >&2; }
fail() { printf '[docker-smoke] FAIL: %s\n' "$*" >&2; exit 1; }

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

command -v docker >/dev/null 2>&1 || fail "docker not found on PATH"
command -v curl   >/dev/null 2>&1 || fail "curl not found on PATH"
[ -f "${SMOKE_CONFIG}" ] || fail "smoke config not found: ${SMOKE_CONFIG}"
docker image inspect "${IMAGE}" >/dev/null 2>&1 \
  || fail "image ${IMAGE} not present — build it first (the CI job does this)"

# Clear leftovers from a previously aborted run.
docker rm -f "${GW_NAME}" "${BE_NAME}" >/dev/null 2>&1 || true
docker network rm "${NET}" >/dev/null 2>&1 || true

log "creating network ${NET}"
docker network create "${NET}" >/dev/null

log "starting backend (${BACKEND_IMAGE}) -> body '${EXPECTED_BODY}'"
docker run -d --name "${BE_NAME}" --network "${NET}" \
  --network-alias "${BE_ALIAS}" \
  "${BACKEND_IMAGE}" \
  -listen=":${BE_PORT}" -text="${EXPECTED_BODY}" >/dev/null \
  || fail "backend failed to start"

# The smoke config is mounted over the image's default config path (argv[1]).
log "starting gateway (${IMAGE}) — config mounted, :${GW_PORT} published to :${HOST_PORT}"
docker run -d --name "${GW_NAME}" --network "${NET}" \
  -p "127.0.0.1:${HOST_PORT}:${GW_PORT}" \
  -v "${SMOKE_CONFIG}:/etc/expressgateway/config.toml:ro" \
  "${IMAGE}" >/dev/null \
  || fail "gateway failed to start"

log "waiting up to ${READY_TIMEOUT}s for the gateway to serve…"
ready=0
for _ in $(seq 1 "${READY_TIMEOUT}"); do
  if [ "$(docker inspect -f '{{.State.Running}}' "${GW_NAME}" 2>/dev/null)" != "true" ]; then
    log "gateway container exited early — logs:"
    docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
    fail "gateway container is not running"
  fi
  # Any non-"000" status means the listener is up; the body is checked later.
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

# The actual proof: a real request THROUGH the gateway.
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

# http-echo appends a trailing newline to -text — match as a substring.
case "${body}" in
  *"${EXPECTED_BODY}"*) : ;;
  *)
    log "body mismatch — gateway logs:"
    docker logs "${GW_NAME}" 2>&1 | sed 's/^/  gw| /' >&2 || true
    fail "response body did not contain backend marker '${EXPECTED_BODY}'"
    ;;
esac

log "verified: 200 + backend body proxied through the running container"
# The EXIT trap prints PASS.
exit 0
