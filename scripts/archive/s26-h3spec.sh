#!/usr/bin/env bash
# S26 h3spec runner. Usage: s26-h3spec.sh <label>
# Boots the migrated expressgateway H3-terminate front (backend-less is fine —
# h3spec conformance is protocol-layer, F-S26-1) and runs h3spec 0.1.13.
# R15: verdict read only from the COMPLETED run (sentinel at end).
set -u
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target
export CARGO_INCREMENTAL=0
LABEL="${1:-preview}"
RUN=/home/ubuntu/h3spec-run
OUT=/home/ubuntu/Code/ExpressGateway/audit/h3spec/s26-h3spec-${LABEL}.log
GWLOG=/home/ubuntu/Code/ExpressGateway/audit/h3spec/s26-gw-${LABEL}.log
CFG=/home/ubuntu/Code/ExpressGateway/audit/h3spec/s26-h3spec-${LABEL}.toml
PORT=28443
BIN=/home/ubuntu/Code/eg-target/debug/expressgateway
cd /home/ubuntu/Code/ExpressGateway || exit 99

# Fresh binary at current tip (no-op if warm).
echo "=== build expressgateway ($(git rev-parse --short HEAD)) ===" | tee "$OUT"
cargo build -p lb --bin expressgateway >> "$OUT" 2>&1 || { echo "BUILD FAIL"; echo "S26-H3SPEC-${LABEL}-DONE rc=98"; exit 98; }

# H3-terminate config (quic listener, raw_proxy ABSENT). Backend block vestigial
# on the quic path (F-S26-1) but kept identical to the S22 template.
cat > "$CFG" <<EOF
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:${PORT}"
protocol = "quic"

[listeners.quic]
cert_path = "${RUN}/cert.pem"
key_path = "${RUN}/key.pem"
retry_secret_path = "${RUN}/retry.bin"

[[listeners.backends]]
address = "127.0.0.1:23000"
protocol = "h1"
weight = 1

[observability]
metrics_bind = "127.0.0.1:29090"
EOF

# Start gateway.
echo "=== boot gateway on :${PORT} ===" | tee -a "$OUT"
"$BIN" "$CFG" > "$GWLOG" 2>&1 &
GW=$!
# Wait for readiness (UDP bind + settle).
for i in $(seq 1 30); do
  if grep -qiE 'listening|quic|ready|accept' "$GWLOG" 2>/dev/null; then break; fi
  if ! kill -0 "$GW" 2>/dev/null; then echo "GW DIED EARLY" | tee -a "$OUT"; tail -20 "$GWLOG" | tee -a "$OUT"; echo "S26-H3SPEC-${LABEL}-DONE rc=97"; exit 97; fi
  sleep 1
done
sleep 2

echo "=== h3spec -n -t 3000 127.0.0.1 ${PORT} ===" | tee -a "$OUT"
/home/ubuntu/.cargo/bin/h3spec -n -t 3000 127.0.0.1 "$PORT" >> "$OUT" 2>&1
H3RC=$?

# Teardown.
kill "$GW" 2>/dev/null; wait "$GW" 2>/dev/null
echo "=== h3spec summary ===" | tee -a "$OUT"
grep -iE 'tests, |passed|failed|skipped' "$OUT" | tail -5 | tee -a "$OUT"
echo "S26-H3SPEC-${LABEL}-DONE rc=${H3RC}" | tee -a "$OUT"
