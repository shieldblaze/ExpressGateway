#!/usr/bin/env bash
# S37-D final self-gate (lead takeover). Settle reqwest->0.12.28 then ×1 + clippy + fmt
# + cargo audit (confirms time-waiver-drop ok) + cargo deny. Baseline >=1562 (B+C).
set -uo pipefail
export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target CARGO_BUILD_JOBS=4
cd /home/ubuntu/Code/ExpressGateway/.claude/worktrees/s37-deps || exit 99
OUT=/home/ubuntu/Code/ExpressGateway/.claude/worktrees/s36-verify/audit/ops/s37-d-lead
mkdir -p "$OUT"
st(){ date -u +%H:%M:%S; }
fg(){ df --output=avail -BG /dev/root|tail -1|tr -dc 0-9; }
echo "[gate $(st)] settle reqwest->0.12.28; disk $(fg)G" | tee "$OUT/gate.console"
cargo update -p reqwest > "$OUT/gate-update.log" 2>&1
echo "[gate $(st)] FINAL resolved (direct deps):" | tee -a "$OUT/gate.console"
for c in tokio socket2 prometheus object aya reqwest time quiche tokio-quiche hyper h2 tokio-tungstenite rustls; do echo -n "  $c: " | tee -a "$OUT/gate.console"; awk -v n="$c" '$1=="name" && $3=="\""n"\""{getline; gsub(/"/,"",$3); printf "%s ",$3}' Cargo.lock | tee -a "$OUT/gate.console"; echo | tee -a "$OUT/gate.console"; done
echo "[gate $(st)] ×1 test --workspace --all-features --no-fail-fast; disk $(fg)G" | tee -a "$OUT/gate.console"
cargo test --workspace --all-features --no-fail-fast > "$OUT/gate-x1.log" 2>&1
X=$?
sums=$(grep -hE "test result:" "$OUT/gate-x1.log" | awk '{p+=$4;f+=$6;ig+=$8} END{printf "passed=%d failed=%d ignored=%d",p,f,ig}')
fails=$(grep -hE "^test .* \.\.\. FAILED" "$OUT/gate-x1.log" | sed 's/ \.\.\. FAILED//' | head -20)
echo "[gate $(st)] ×1 rc=$X $sums; disk $(fg)G" | tee -a "$OUT/gate.console"
[ -n "$fails" ] && { echo "[gate $(st)] FAILED:" | tee -a "$OUT/gate.console"; echo "$fails" | tee -a "$OUT/gate.console"; }
echo "[gate $(st)] clippy" | tee -a "$OUT/gate.console"
cargo clippy --workspace --all-targets --all-features -- -D warnings > "$OUT/gate-clippy.log" 2>&1; C=$?
echo "[gate $(st)] clippy rc=$C ($(tail -1 "$OUT/gate-clippy.log"))" | tee -a "$OUT/gate.console"
cargo fmt --all -- --check > "$OUT/gate-fmt.log" 2>&1; F=$?
echo "[gate $(st)] fmt rc=$F" | tee -a "$OUT/gate.console"
if command -v cargo-audit >/dev/null 2>&1; then cargo audit -D warnings > "$OUT/gate-audit.log" 2>&1; A=$?; echo "[gate $(st)] audit rc=$A ($(grep -hE 'error|warning|vulnerabilit' "$OUT/gate-audit.log" | head -1))" | tee -a "$OUT/gate.console"; else A=skip; echo "[gate $(st)] cargo-audit not installed (CI binding)" | tee -a "$OUT/gate.console"; fi
if command -v cargo-deny >/dev/null 2>&1; then cargo deny check > "$OUT/gate-deny.log" 2>&1; DN=$?; echo "[gate $(st)] deny rc=$DN ($(grep -hcE 'error\[' "$OUT/gate-deny.log") errors)" | tee -a "$OUT/gate.console"; else DN=skip; echo "[gate $(st)] cargo-deny not installed (CI binding)" | tee -a "$OUT/gate.console"; fi
echo "DONE x1_rc=$X $sums clippy=$C fmt=$F audit=$A deny=$DN $(date -u)" > "$OUT/gate.marker"
echo "[gate $(st)] ALL DONE x1=$X clippy=$C fmt=$F audit=$A deny=$DN" | tee -a "$OUT/gate.console"