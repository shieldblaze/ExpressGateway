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
package com.shieldblaze.expressgateway.protocol.quic;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Maps QUIC Connection IDs (CID) to backend sessions for CID-based session affinity.
 *
 * <p>QUIC connections are identified by Connection IDs rather than the 4-tuple
 * (source IP, source port, dest IP, dest port). When a client migrates its network
 * path (e.g., WiFi to cellular), the source address changes but the CID stays the
 * same (RFC 9000 Section 9). This map enables the proxy to route the migrated
 * connection to the same backend, preserving the QUIC session.</p>
 *
 * <h3>Idle Eviction</h3>
 * <p>Entries are evicted when not accessed within the configured idle timeout.
 * Access is refreshed on every {@link #get(byte[])} hit. A background daemon thread
 * runs periodic sweeps to remove stale entries.</p>
 *
 * <h3>Short Header CID Length Tracking</h3>
 * <p>Short Header packets (1-RTT) do not encode the CID length -- the receiver must
 * know it. This map tracks the distinct CID lengths of active sessions via
 * {@link #knownCidLengths()}, enabling the proxy to probe a small set of lengths
 * when parsing Short Headers from unknown sources.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>All operations are safe for concurrent access from multiple EventLoop threads.
 * The backing {@link ConcurrentHashMap} provides per-bin locking.</p>
 */
public final class QuicCidSessionMap implements Closeable {

    private static final Logger logger = LogManager.getLogger(QuicCidSessionMap.class);

    private final ConcurrentHashMap<ByteArrayKey, SessionEntry> map = new ConcurrentHashMap<>();
    private final Duration idleTimeout;
    private final ScheduledFuture<?> evictionTask;

    /**
     * Tracks the count of active sessions per CID length. Used to provide the set
     * of known CID lengths for Short Header parsing. Typically contains 1-3 entries
     * since most deployments use a single CID length.
     */
    private final ConcurrentHashMap<Integer, Integer> cidLengthCounts = new ConcurrentHashMap<>();

    // PERF-5: Cached knownCidLengths result. CID lengths change rarely (typically 1 value),
    // so we cache the int[] and invalidate on put/remove instead of allocating a Stream
    // pipeline per Short Header packet.
    private volatile int[] cachedCidLengths = new int[0];

    /**
     * Internal entry holding the session, its CID length, and the last access timestamp.
     * Immutable -- a new instance is created on refresh to avoid torn reads.
     */
    private record SessionEntry(QuicBackendSession session, int cidLength, long lastAccessNanos) {
        SessionEntry withRefreshedAccess() {
            return new SessionEntry(session, cidLength, System.nanoTime());
        }
    }

    /**
     * Create a new CID session map with the given idle timeout.
     *
     * @param idleTimeout duration after which entries with no activity are evicted
     */
    public QuicCidSessionMap(Duration idleTimeout) {
        this.idleTimeout = idleTimeout;
        // PERF-3: Use shared static executor from QuicConnectionPool instead of
        // creating a per-instance ScheduledExecutorService. Prevents thread proliferation
        // when multiple QuicCidSessionMap instances are created.
        long intervalSeconds = Math.max(1, idleTimeout.toSeconds());
        this.evictionTask = QuicConnectionPool.SHARED_EVICTION_EXECUTOR.scheduleAtFixedRate(
                this::evictExpired, intervalSeconds, intervalSeconds, TimeUnit.SECONDS);
    }

    /**
     * Look up a backend session by Connection ID.
     * Refreshes the entry's last-access timestamp on hit.
     *
     * @param cid the QUIC Connection ID bytes
     * @return the associated session, or {@code null} if not found
     */
    public QuicBackendSession get(byte[] cid) {
        ByteArrayKey key = new ByteArrayKey(cid);
        SessionEntry entry = map.get(key);
        if (entry != null) {
            // Refresh last-access timestamp
            map.put(key, entry.withRefreshedAccess());
            return entry.session();
        }
        return null;
    }

    /**
     * Register a CID-to-session mapping.
     *
     * @param cid       the QUIC Connection ID bytes
     * @param session   the backend session to route to
     * @param cidLength the length of this CID (tracked for Short Header parsing)
     */
    public void put(byte[] cid, QuicBackendSession session, int cidLength) {
        map.put(new ByteArrayKey(cid), new SessionEntry(session, cidLength, System.nanoTime()));
        cidLengthCounts.merge(cidLength, 1, Integer::sum);
        invalidateCidLengthCache();
    }

    /**
     * Remove a CID mapping (e.g., when the session expires or closes).
     *
     * @param cid the QUIC Connection ID bytes
     */
    public void remove(byte[] cid) {
        SessionEntry removed = map.remove(new ByteArrayKey(cid));
        if (removed != null) {
            decrementCidLengthCount(removed.cidLength());
            invalidateCidLengthCache();
        }
    }

    /**
     * Returns the set of distinct CID lengths from active sessions.
     * Typically 1-3 values. Used to probe Short Header CID extraction
     * when the CID length is unknown for a given packet.
     *
     * @return array of known CID lengths (may be empty)
     */
    public int[] knownCidLengths() {
        return cachedCidLengths;
    }

    private void invalidateCidLengthCache() {
        // Avoid Stream API allocation: cidLengthCounts typically has 1-3 entries
        var keys = cidLengthCounts.keySet();
        int[] result = new int[keys.size()];
        int i = 0;
        for (Integer key : keys) {
            if (i < result.length) {
                result[i++] = key;
            }
        }
        // Trim if concurrent removal shrank the keyset between size() and iteration
        cachedCidLengths = (i == result.length) ? result : java.util.Arrays.copyOf(result, i);
    }

    /**
     * Returns the number of active CID mappings.
     */
    public int size() {
        return map.size();
    }

    private void evictExpired() {
        try {
            long now = System.nanoTime();
            long timeoutNanos = idleTimeout.toNanos();
            boolean[] anyEvicted = {false};
            map.entrySet().removeIf(e -> {
                if (now - e.getValue().lastAccessNanos() > timeoutNanos) {
                    decrementCidLengthCount(e.getValue().cidLength());
                    anyEvicted[0] = true;
                    if (logger.isDebugEnabled()) {
                        logger.debug("Evicted expired CID mapping: {}", e.getKey());
                    }
                    return true;
                }
                return false;
            });
            // Invalidate cache if any entries were evicted, since CID length bucket
            // counts may have changed (a bucket may have been removed via
            // decrementCidLengthCount returning null)
            if (anyEvicted[0]) {
                invalidateCidLengthCache();
            }
        } catch (Exception e) {
            logger.error("Error during CID session eviction sweep", e);
        }
    }

    private void decrementCidLengthCount(int cidLength) {
        cidLengthCounts.computeIfPresent(cidLength, (k, v) -> v <= 1 ? null : v - 1);
    }

    @Override
    public void close() {
        evictionTask.cancel(false);
        // PERF-3: Do NOT shut down the shared executor -- it is static and shared
        // across all QUIC pool and CID map instances.
        map.clear();
        cidLengthCounts.clear();
        cachedCidLengths = new int[0];
    }
}
