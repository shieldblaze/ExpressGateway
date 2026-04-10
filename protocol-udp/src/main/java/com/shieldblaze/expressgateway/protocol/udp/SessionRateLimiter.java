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
package com.shieldblaze.expressgateway.protocol.udp;

import lombok.extern.log4j.Log4j2;

import java.net.InetAddress;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Per-source rate limiter for UDP datagrams using a token bucket algorithm.
 *
 * <p>Each unique source IP address gets its own token bucket. Tokens are replenished
 * lazily on each {@link #tryAcquire(InetAddress)} call using the elapsed time since
 * the last replenishment. This avoids requiring a background timer thread.</p>
 *
 * <p>Design decisions:
 * <ul>
 *   <li>Keyed by IP address (not IP:port) to prevent amplification attacks that
 *       vary source ports.</li>
 *   <li>Lock-free per-bucket via CAS on AtomicLong (tokens encoded as fixed-point).</li>
 *   <li>Stale buckets are evicted by {@link #evictStale(long)} to prevent unbounded
 *       map growth under diverse source traffic.</li>
 * </ul></p>
 */
@Log4j2
final class SessionRateLimiter {

    /**
     * Rate limiter configuration.
     *
     * @param packetsPerSecond maximum sustained packet rate per source IP
     * @param burstSize        maximum burst size (token bucket capacity)
     */
    record Config(long packetsPerSecond, long burstSize) {

        /**
         * Disabled rate limiter -- allows all traffic through.
         */
        static final Config DISABLED = new Config(0, 0);

        /**
         * Default: 10,000 packets/sec per source with burst of 1,000.
         * Suitable for most UDP proxy workloads (DNS, game servers, VPN).
         */
        static final Config DEFAULT = new Config(10_000, 1_000);

        Config {
            if (packetsPerSecond < 0) throw new IllegalArgumentException("packetsPerSecond must be >= 0");
            if (burstSize < 0) throw new IllegalArgumentException("burstSize must be >= 0");
        }

        boolean isDisabled() {
            return packetsPerSecond == 0;
        }
    }

    /**
     * Per-source token bucket state. Uses a single AtomicLong to store tokens
     * (scaled by TOKEN_SCALE for sub-packet precision) and an AtomicLong for
     * the last refill timestamp in nanoseconds (CAS-protected to prevent
     * double-crediting when multiple threads refill concurrently).
     */
    private static final class Bucket {
        private final AtomicLong tokens;
        private final AtomicLong lastRefillNanos;

        Bucket(long initialTokens) {
            this.tokens = new AtomicLong(initialTokens * TOKEN_SCALE);
            this.lastRefillNanos = new AtomicLong(System.nanoTime());
        }
    }

    /**
     * Fixed-point scale factor for sub-packet precision in token accounting.
     * Allows fractional token accumulation without floating-point arithmetic.
     */
    private static final long TOKEN_SCALE = 1_000_000L;

    private final ConcurrentHashMap<InetAddress, Bucket> buckets;
    private final Config config;
    private final LongAdder totalDropped = new LongAdder();

    SessionRateLimiter(Config config) {
        this.config = config;
        this.buckets = new ConcurrentHashMap<>();
    }

    SessionRateLimiter() {
        this(Config.DEFAULT);
    }

    /**
     * Try to acquire a permit for one datagram from the given source.
     *
     * @param source the source IP address
     * @return true if the datagram is allowed, false if rate-limited
     */
    boolean tryAcquire(InetAddress source) {
        if (config.isDisabled()) {
            return true;
        }

        Bucket bucket = buckets.computeIfAbsent(source, _ -> new Bucket(config.burstSize()));
        refill(bucket);

        // CAS loop to atomically consume one token
        while (true) {
            long current = bucket.tokens.get();
            if (current < TOKEN_SCALE) {
                totalDropped.increment();
                return false;
            }
            long next = current - TOKEN_SCALE;
            if (bucket.tokens.compareAndSet(current, next)) {
                return true;
            }
            // CAS failed -- another thread consumed tokens concurrently. Retry.
        }
    }

    /**
     * Lazily refill tokens based on elapsed time since last refill.
     * Uses CAS on lastRefillNanos to prevent double-crediting when
     * multiple threads refill the same bucket concurrently.
     */
    private void refill(Bucket bucket) {
        long lastRefill = bucket.lastRefillNanos.get();
        long now = System.nanoTime();
        long elapsed = now - lastRefill;
        if (elapsed <= 0) {
            return;
        }

        // Calculate new tokens using millisecond precision to handle sub-second refills
        // without overflow: (elapsed_ms * pps / 1000) * TOKEN_SCALE.
        long elapsedMs = elapsed / 1_000_000L;
        long newTokens = (elapsedMs * config.packetsPerSecond() / 1_000L) * TOKEN_SCALE;

        if (newTokens > 0) {
            // CAS the timestamp first -- only the winner adds tokens, preventing double-credit.
            if (!bucket.lastRefillNanos.compareAndSet(lastRefill, now)) {
                return;
            }
            long maxTokens = config.burstSize() * TOKEN_SCALE;
            // CAS loop to add tokens without exceeding capacity
            while (true) {
                long current = bucket.tokens.get();
                long updated = Math.min(current + newTokens, maxTokens);
                if (updated == current || bucket.tokens.compareAndSet(current, updated)) {
                    break;
                }
            }
        }
    }

    /**
     * Evict stale buckets that haven't been touched in the given duration (nanoseconds).
     * Should be called periodically by a scheduled task to prevent unbounded map growth.
     *
     * @param staleThresholdNanos buckets idle longer than this are removed
     * @return number of buckets evicted
     */
    int evictStale(long staleThresholdNanos) {
        int evicted = 0;
        long now = System.nanoTime();
        var it = buckets.entrySet().iterator();
        while (it.hasNext()) {
            var entry = it.next();
            if (now - entry.getValue().lastRefillNanos.get() > staleThresholdNanos) {
                it.remove();
                evicted++;
            }
        }
        return evicted;
    }

    /**
     * Total number of datagrams dropped across all sources since creation.
     */
    long totalDropped() {
        return totalDropped.sum();
    }

    /**
     * Number of actively tracked source addresses.
     */
    int trackedSources() {
        return buckets.size();
    }

    Config config() {
        return config;
    }
}
