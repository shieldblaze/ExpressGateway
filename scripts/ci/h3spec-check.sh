#!/usr/bin/env bash
#
# D-4 h3spec conformance gate with an EXPLICIT, NAMED waiver list (S34).
#
# h3spec (kazu-yamamoto, version pinned in ci.yml) runs 49 HTTP/3 + QUIC conformance
# examples against the booted gateway. 12 fail on KNOWN quiche-0.29 limitations tracked
# as CF-QUICHE-UPGRADE: quiche reads-and-discards (or does not validate) certain
# malformed QUIC transport parameters / reserved bits, plus the two QPACK
# encoder/decoder-stream instruction errors. Upstream-quiche deviations, NOT gateway
# bugs — our own H3/QPACK pseudo-header, frame-sequencing and message-error handling
# all PASS.
#
# HONESTY CONTRACT — this is NOT a blanket "h3spec failures allowed", which would make
# the green meaningless. Each waiver is named individually with its spec reference, and
# the gate passes iff the failure set is a SUBSET of that list AND the suite actually
# ran (>= MIN_EXAMPLES). An un-waived failure is a real conformance regression -> RED,
# never silently swallowed. A waiver that starts passing only WARNs with a prune hint,
# so an upstream fix cannot break CI.
#
# Usage: h3spec-check.sh <h3spec-binary> <host> <port> [extra h3spec args...]
# -n = do not validate the server cert; this is PROTOCOL conformance (h2spec's -k
# analog), PKI is out of scope.

set -uo pipefail

H3SPEC="${1:?usage: h3spec-check.sh <h3spec-bin> <host> <port>}"
HOST="${2:?host required}"
PORT="${3:?port required}"
shift 3 || true

# Guards against "h3spec couldn't connect so 0 ran" masquerading as success.
# h3spec v0.1.13 ships 49 examples.
MIN_EXAMPLES=40

# The 12 named waivers (CF-QUICHE-UPGRADE). Strings must match h3spec's description
# EXACTLY. First 10: transport-parameter validation quiche 0.29 does not enforce.
WAIVERS=(
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if initial_source_connection_id is missing [Transport 7.3]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if original_destination_connection_id is received [Transport 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if preferred_address, is received [Transport 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if retry_source_connection_id is received [Transport 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if stateless_reset_token is received [Transport 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if max_udp_payload_size < 1200 [Transport 7.4 and 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if ack_delay_exponen > 20 [Transport 7.4 and 18.2]"
  "QUIC servers MUST send TRANSPORT_PARAMETER_ERROR if max_ack_delay >= 2^14 [Transport 7.4 and 18.2]"
  "QUIC servers MUST send PROTOCOL_VIOLATION if reserved bits in Handshake are non-zero [Transport 17.2]"
  "QUIC servers MUST send PROTOCOL_VIOLATION if reserved bits in Short are non-zero [Transport 17.2]"
  # QPACK encoder/decoder-stream instructions quiche reads-and-discards (h3spec #23/#25).
  "HTTP/3 servers MUST send QPACK_ENCODER_STREAM_ERROR if a new dynamic table capacity value exceeds the limit [QPACK 4.1.3]"
  "HTTP/3 servers MUST send QPACK_DECODER_STREAM_ERROR if Insert Count Increment is 0 [QPACK 4.4.3]"
)

is_waived() {
  local desc="$1" w
  for w in "${WAIVERS[@]}"; do
    [ "$desc" = "$w" ] && return 0
  done
  return 1
}

OUT="$(mktemp)"
echo ">>> h3spec -n -t 5000 $* $HOST $PORT"
"$H3SPEC" -n -t 5000 "$@" "$HOST" "$PORT" >"$OUT" 2>&1
H3_RC=$?
cat "$OUT"
echo "--------------------------------------------------------------------"

EXAMPLES="$(grep -oE '[0-9]+ examples' "$OUT" | tail -1 | grep -oE '[0-9]+' || echo 0)"
if [ "${EXAMPLES:-0}" -lt "$MIN_EXAMPLES" ]; then
  echo "::error::h3spec ran only ${EXAMPLES} examples (< ${MIN_EXAMPLES}); the suite did not"
  echo "         execute properly (gateway boot / H3 reachability problem). Failing."
  rm -f "$OUT"; exit 1
fi

# Failure descriptions, stripped of h3spec's "  N) " prefix.
mapfile -t FAILURES < <(grep -E '^[[:space:]]+[0-9]+\)' "$OUT" | sed -E 's/^[[:space:]]+[0-9]+\) //')

UNEXPECTED=()
for f in "${FAILURES[@]}"; do
  is_waived "$f" || UNEXPECTED+=("$f")
done

# Waivers that did NOT fire = quiche fixed them = prune candidates.
NOT_HIT=()
for w in "${WAIVERS[@]}"; do
  hit=0
  for f in "${FAILURES[@]}"; do [ "$f" = "$w" ] && hit=1 && break; done
  [ "$hit" -eq 0 ] && NOT_HIT+=("$w")
done

echo "h3spec: ${EXAMPLES} examples, ${#FAILURES[@]} failures, ${#WAIVERS[@]} named waivers (CF-QUICHE-UPGRADE)."

if [ "${#NOT_HIT[@]}" -gt 0 ]; then
  echo "::warning::${#NOT_HIT[@]} waived case(s) now PASS — quiche may have fixed them; prune the waiver list:"
  for w in "${NOT_HIT[@]}"; do echo "    (now passing) $w"; done
fi

if [ "${#UNEXPECTED[@]}" -gt 0 ]; then
  echo "::error::${#UNEXPECTED[@]} UNEXPECTED h3spec failure(s) NOT on the CF-QUICHE-UPGRADE waiver list:"
  for u in "${UNEXPECTED[@]}"; do echo "    (UNEXPECTED) $u"; done
  echo "::error::A new/un-waived h3spec failure is a real conformance regression. Failing D-4."
  rm -f "$OUT"; exit 1
fi

echo "PASS: every h3spec failure is an individually-named, documented quiche-0.29"
echo "      limitation (CF-QUICHE-UPGRADE). ${EXAMPLES} examples ran; the"
echo "      $(( EXAMPLES - ${#FAILURES[@]} )) non-waived examples all passed."
rm -f "$OUT"
exit 0
