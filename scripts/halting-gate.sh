#!/usr/bin/env bash
# Halting gate — pure function of repo state. Exit 0 = GREEN, exit 1 = RED.
set -u
cd "$(git rev-parse --show-toplevel)"

fail() { echo "RED: $1"; echo "REMEDIATION: $2"; exit 1; }

# Check 1 — toolchain & format
command -v cargo >/dev/null || fail "cargo missing" "install rust toolchain per rust-toolchain.toml"
cargo fmt --check >/dev/null 2>&1 || fail "cargo fmt --check failed" "run 'cargo fmt' and commit"

# Check 2 — clippy with project deny-lints
cargo clippy --all-targets --all-features -- -D warnings >/dev/null 2>&1 \
  || fail "cargo clippy failed" "run 'cargo clippy --all-targets -- -D warnings' and fix the first error only"

# Check 3 — panic grep (Cloudflare 2025 outage rule)
# Use awk to skip #[cfg(test)] module blocks (brace-balanced) and comment lines.
panic_hits=$(find crates/ -name '*.rs' -not -path '*/tests/*' -print0 | xargs -0 awk '
  /^[[:space:]]*\/\// { next }
  /#\[cfg\(test\)\]/ { in_test=1; depth=0; next }
  in_test && /{/ { depth++; next }
  in_test && /}/ { depth--; if(depth<=0){in_test=0}; next }
  in_test { next }
  /unwrap\(\)|\.expect\(|panic!|todo!|unimplemented!|unreachable!/ { print FILENAME":"NR": "$0 }
' 2>/dev/null)
if [ -n "$panic_hits" ]; then
  echo "$panic_hits" | head -5
  fail "panic-prone construct in non-test code" "replace with Result; first offender shown above"
fi

# Check 4 — required artifacts (closed list from §4.1)
missing=0
while IFS= read -r f; do
  [ -z "$f" ] && continue
  if [ ! -f "$f" ]; then echo "MISSING ARTIFACT: $f"; missing=1; fi
done < manifest/required-artifacts.txt
[ $missing -eq 0 ] || fail "required artifacts missing" "create the first missing file per §4 spec; do not create anything else"

# Check 5 — required tests all present and passing
cargo test --all --all-features --no-fail-fast 2>&1 | tee target/test-output.log >/dev/null \
  || fail "cargo test failed" "fix the first failing test shown in target/test-output.log"
while IFS= read -r t; do
  [ -z "$t" ] && continue
  grep -q "test $t \.\.\. ok" target/test-output.log \
    || fail "required test missing or failing: $t" "implement test $t per §4.2; do not touch other tests"
done < manifest/required-tests.txt

# Check 6 — cargo-deny
cargo deny check >/dev/null 2>&1 || fail "cargo deny failed" "fix the first deny.toml violation; do not relax deny.toml"

# Check 7 — manifest integrity (prevents silent scope creep)
cat manifest/required-artifacts.txt manifest/required-tests.txt | sort | sha256sum -c .halting-gate.sha256 --quiet \
  || fail "manifest hash mismatch" "manifest has drifted; open docs/manifest-drift-proposal.md and STOP"

echo "PROJECT COMPLETE — halting gate green."
echo "Artifacts: $(wc -l < manifest/required-artifacts.txt)/$(wc -l < manifest/required-artifacts.txt).  Tests: $(wc -l < manifest/required-tests.txt)/$(wc -l < manifest/required-tests.txt).  Manifest: OK."
echo "No work to do. Exiting."
exit 0
