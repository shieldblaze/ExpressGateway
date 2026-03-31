/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.quic.zerortt;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Strike register for 0-RTT replay detection per RFC 9001 Section 4.7.
 *
 * <h3>Problem</h3>
 * <p>0-RTT data (early data) sent during the QUIC handshake can be replayed by an
 * attacker who captures the TLS ClientHello containing the PSK and early data.
 * The server cannot distinguish a legitimate 0-RTT request from a replayed one
 * using TLS alone, because the 0-RTT key material is derived from the PSK which
 * is reusable.</p>
 *
 * <h3>Solution: Time-Windowed Strike Register</h3>
 * <p>This detector maintains a time-windowed set of 0-RTT identifiers (derived from
 * the client's TLS session ticket + timestamp). The window slides forward as time
 * advances. Identifiers older than the window are automatically expired.</p>
 *
 * <h3>Memory Bounding</h3>
 * <p>The register is bounded in two dimensions:
 * <ul>
 *   <li>Time: entries expire after the configured window duration</li>
 *   <li>Count: a hard cap on entries prevents memory exhaustion under attack.
 *       When the cap is reached, all 0-RTT is rejected until entries expire.</li>
 * </ul></p>
 *
 * <h3>Implementation: Double-Buffered ConcurrentHashMap</h3>
 * <p>Uses two ConcurrentHashMaps alternating between "current" and "previous" windows.
 * When the current window expires, "previous" is discarded and "current" becomes
 * "previous", while a fresh map becomes "current". This avoids per-entry expiry
 * overhead and provides O(1) amortized cleanup.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>All operations are lock-free. The window rotation uses {@link AtomicReference}
 * CAS to swap buffers without locking.</p>
 */
public final class ReplayDetector {

    private static final Logger logger = LogManager.getLogger(ReplayDetector.class);

    /**
     * An identifier entry in the strike register.
     * Uses nanoTime for monotonic timing (immune to wall-clock adjustments).
     */
    private record StrikeEntry(long insertedAtNanos) {
    }

    /**
     * Double-buffered window state. Holds the current and previous window maps
     * and the timestamp when the current window started.
     */
    private record WindowState(
            ConcurrentHashMap<Long, StrikeEntry> current,
            ConcurrentHashMap<Long, StrikeEntry> previous,
            long windowStartNanos
    ) {
    }

    private final long windowDurationNanos;
    private final int maxEntries;

    private final AtomicReference<WindowState> stateRef;
    private final AtomicLong totalChecks = new AtomicLong();
    private final AtomicLong replaysDetected = new AtomicLong();
    private final AtomicLong entriesRegistered = new AtomicLong();
    private final AtomicLong capacityRejections = new AtomicLong();

    /**
     * Create a replay detector with default settings.
     * Window: 10 seconds, max 100,000 entries.
     */
    public ReplayDetector() {
        this(10_000_000_000L, 100_000);
    }

    /**
     * Create a replay detector with custom settings.
     *
     * @param windowDurationNanos time window in nanoseconds. 0-RTT requests older
     *                            than this are allowed (the replay risk has passed).
     * @param maxEntries          maximum entries before all 0-RTT is rejected.
     */
    public ReplayDetector(long windowDurationNanos, int maxEntries) {
        if (windowDurationNanos <= 0) {
            throw new IllegalArgumentException("windowDurationNanos must be positive");
        }
        if (maxEntries <= 0) {
            throw new IllegalArgumentException("maxEntries must be positive");
        }
        this.windowDurationNanos = windowDurationNanos;
        this.maxEntries = maxEntries;

        WindowState initial = new WindowState(
                new ConcurrentHashMap<>(),
                new ConcurrentHashMap<>(),
                System.nanoTime()
        );
        this.stateRef = new AtomicReference<>(initial);
    }

    /**
     * Check if a 0-RTT identifier is a replay and register it if not.
     *
     * <p>This is an atomic check-and-register operation. If the identifier
     * has been seen within the current or previous window, it is a replay.
     * Otherwise, it is registered in the current window.</p>
     *
     * @param identifier a unique identifier for this 0-RTT attempt. Typically
     *                   derived from: hash(client_ticket + client_random + early_data_hash)
     * @return true if this is a REPLAY (identifier was already seen), false if fresh
     */
    public boolean isReplay(long identifier) {
        totalChecks.incrementAndGet();
        WindowState state = maybeRotateWindow();

        // Check capacity -- if we are at max, reject all 0-RTT to be safe
        int totalSize = state.current().size() + state.previous().size();
        if (totalSize >= maxEntries) {
            capacityRejections.incrementAndGet();
            return true;
        }

        // Check if seen in current window
        if (state.current().containsKey(identifier)) {
            replaysDetected.incrementAndGet();
            return true;
        }

        // Check if seen in previous window (still within the replay danger zone)
        if (state.previous().containsKey(identifier)) {
            replaysDetected.incrementAndGet();
            return true;
        }

        // Not seen -- register in current window
        StrikeEntry existing = state.current().putIfAbsent(identifier, new StrikeEntry(System.nanoTime()));
        if (existing != null) {
            // Another thread registered it between our check and putIfAbsent
            replaysDetected.incrementAndGet();
            return true;
        }

        entriesRegistered.incrementAndGet();
        return false;
    }

    /**
     * Check if an identifier would be considered a replay WITHOUT registering it.
     * Useful for dry-run checks or logging.
     *
     * @param identifier the 0-RTT identifier
     * @return true if the identifier has been seen
     */
    public boolean wouldBeReplay(long identifier) {
        WindowState state = maybeRotateWindow();
        return state.current().containsKey(identifier) || state.previous().containsKey(identifier);
    }

    /**
     * Compute a 0-RTT identifier from the component parts.
     *
     * <p>Uses a mixing function to combine the ticket hash and a nonce into
     * a single long identifier. This is not a cryptographic hash but provides
     * sufficient distribution for the strike register's HashMap.</p>
     *
     * @param ticketHash hash of the TLS session ticket
     * @param nonce      a per-connection nonce (e.g., from client random)
     * @return a combined identifier
     */
    public static long computeIdentifier(long ticketHash, long nonce) {
        // Two-input mixing: combine inputs asymmetrically to avoid collision when a^b == c^d.
        // First, separate the inputs with distinct constants, then mix with Stafford variant 13.
        long z = (ticketHash * 0x9E3779B97F4A7C15L) + (nonce * 0x6A09E667F3BCC908L);
        z = (z ^ (z >>> 30)) * 0xbf58476d1ce4e5b9L;
        z = (z ^ (z >>> 27)) * 0x94d049bb133111ebL;
        return z ^ (z >>> 31);
    }

    /**
     * Returns the current number of entries across both windows.
     */
    public int size() {
        WindowState state = stateRef.get();
        return state.current().size() + state.previous().size();
    }

    public long totalChecks() {
        return totalChecks.get();
    }

    public long replaysDetected() {
        return replaysDetected.get();
    }

    public long entriesRegistered() {
        return entriesRegistered.get();
    }

    public long capacityRejections() {
        return capacityRejections.get();
    }

    /**
     * Rotate the window if the current window has expired.
     * Uses CAS to ensure only one thread performs the rotation.
     *
     * @return the (possibly rotated) current state
     */
    private WindowState maybeRotateWindow() {
        WindowState current = stateRef.get();
        long now = System.nanoTime();

        if (now - current.windowStartNanos() < windowDurationNanos) {
            return current;
        }

        // Window expired -- rotate: current becomes previous, create new current
        WindowState rotated = new WindowState(
                new ConcurrentHashMap<>(),
                current.current(),
                now
        );

        // CAS: only one thread rotates. Losers get the winner's state.
        if (stateRef.compareAndSet(current, rotated)) {
            int discarded = current.previous().size();
            if (discarded > 0 && logger.isDebugEnabled()) {
                logger.debug("Replay detector window rotated, discarded {} expired entries", discarded);
            }
            return rotated;
        }

        // Another thread won the CAS -- return their state
        return stateRef.get();
    }
}
