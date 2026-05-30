# S19 — B4 Plan: datagram relay + bounded drop-newest queue

Status: APPROVED design (lead-authored, lead-approved per R5). builder-1
implements; verifier verifies independently. No source change before
Phase 0 green is confirmed.

## Goal (per S16 Mode B plan §2.4 / R8)

Forward RFC 9221 unreliable datagrams **verbatim** through the two-connection
Mode B relay, both directions. Per-connection datagram queue **bounded** with
an **explicit drop-newest** full-policy. The bound is the R8 memory-safety
mechanism (not a total/body cap).

## quiche 0.28 datagram semantics (confirmed from source)

- `enable_dgram(true, recv_queue_len, send_queue_len)` advertises
  `max_datagram_frame_size` and bounds quiche's own recv/send queues.
- `dgram_recv(buf) -> Ok(len) | Err(Done) | Err(BufferTooShort)`:
  pops one datagram off quiche's recv queue. `Done` = none left.
- `dgram_send(buf) -> Ok(()) | Err(Done) | Err(InvalidState) | Err(BufferTooShort)`:
  `Done` = quiche send queue full (caller decides drop/retry);
  `InvalidState` = peer never negotiated DATAGRAM;
  `BufferTooShort` = datagram exceeds peer's max writable len.
- quiche's recv queue itself is already drop-NEWEST on overflow (push returns
  `Err(Done)` and the arriving frame is discarded) — our explicit queue mirrors
  that policy at the relay layer so the behaviour is owned, documented, and
  unit-testable.

## Design

### 1. `BoundedDgramQueue` (the explicit bound + policy point)

New small type in `raw_proxy.rs`:

```
struct BoundedDgramQueue {
    q: VecDeque<Vec<u8>>,   // FIFO of datagram payloads (verbatim bytes)
    cap: usize,             // max items; the R8 bound
    dropped: u64,           // drop-newest events (surfaced for B6 metric)
}
```

- `push(payload)`: if `q.len() >= cap` → **drop-newest**: discard `payload`,
  `dropped += 1`, return `Dropped`. Else `q.push_back(payload)`, return `Queued`.
- `front()/pop_front()/len()/is_empty()` as needed.
- Payload stored as owned `Vec<u8>` = verbatim bytes (binary-safe).

`cap` default `DGRAM_QUEUE_CAP = 1024` (matches quiche's queue default; industry-
safe per R7 pre-auth; documented). Per-connection per-direction worst-case
memory = `cap * max_datagram_size`, bounded and independent of total traffic.
Two queues per connection-pair (c2u, u2c), created in `run_dual_pump` next to
the stream table.

### 2. `relay_datagrams(client, upstream, c2u_q, u2c_q)`

Called in `run_dual_pump`'s loop **immediately after `relay_streams(...)`**
(raw_proxy.rs:463). Symmetric, one helper `pump_dgram_dir(src, dst, q)` per
direction:

- **Recv-drain** `src`: loop `src.dgram_recv(&mut buf)` (buf sized to a
  max-datagram constant):
  - `Ok(len)` → `q.push(buf[..len].to_vec())` (drop-newest if full).
  - `Err(Done)` → break.
  - `Err(BufferTooShort)` → unreachable with max buf; log + break (defensive).
- **Send-drain** to `dst`: while `q.front()` is `Some`:
  - `dst.dgram_send(front)`:
    - `Ok(())` → `q.pop_front()`.
    - `Err(Done)` → dst send queue full → **stop this turn** (leave queued;
      bounded by cap; retried next wake). break.
    - `Err(BufferTooShort)` → datagram too large for dst's max writable →
      drop THIS one (`pop_front`, count), continue (cannot ever forward it).
    - `Err(InvalidState)` → dst never negotiated DATAGRAM → drop + log; this
      direction cannot forward (only if mis-wired; negotiation is checked at
      config time).

Datagrams are connectionless w.r.t. streams: they have no FIN, no reset, no
ordering guarantee — so the relay never touches stream state and never blocks
the stream relay. No change to `relay_streams`/`pump_dir`.

### 3. Negotiation / config

B4's relay logic is exercised on the wire via the existing loopback rig
(`terminate_loopback.rs` already `enable_dgram(true, 1024, 1024)` on both
configs). Production config `enable_dgram` (client-facing `build_server_config`
for Mode-B listeners + backend pool `config_factory`) is **B6 wiring** — keep
the H3-terminate path byte-identical (R3): only Mode-B listeners enable
datagrams. B4 does NOT modify `build_server_config`.

## Verification (verifier, independent)

- **Real-wire pass-through**: client sends N binary datagrams (incl. zero-length
  and max-size, non-UTF8 bytes) → assert backend receives byte-identical, and
  reverse direction backend→client. Two distinct connections (Mode B).
- **Queue bound under flood (R8)**: flood datagrams faster than dst drains;
  assert OUR queue never exceeds `cap` and process memory does not grow
  unbounded; `dropped` counter increments.
- **R13 (a)** inside the ×3 workspace gate (integration test).
- **R13 (b)** isolation-burst ≥50 iters of the drop-newest path (unit test on
  `BoundedDgramQueue` flooded + the wire flood test looped).
- **R13 (c)** LOAD-BEARING NEGATIVE CONTROL: a test that pushes `cap + K` items
  and asserts `len() == cap`, the K newest were dropped, and the oldest `cap`
  survived in order. An unbounded queue (the pre-fix shape) fails this; the
  bounded drop-newest passes. Mechanism-proven, not asserted.

## Files touched (B4)

- `crates/lb-quic/src/raw_proxy.rs`: `BoundedDgramQueue`, `relay_datagrams`,
  `pump_dgram_dir`, wire into `run_dual_pump`; unit tests for the queue.
- `crates/lb-quic/tests/`: new real-wire datagram pass-through + flood +
  drop-newest integration test (mirrors the S16 loopback test shape).

## Out of scope for B4 (carried to B6)

- Production `enable_dgram` on Mode-B client/backend configs + main.rs wiring.
- `quic_modeb_datagrams_*` metric surface (the `dropped` counter is plumbed so
  B6 can expose it).
