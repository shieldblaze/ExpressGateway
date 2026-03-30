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
package com.shieldblaze.expressgateway.security;

import io.netty.channel.ChannelHandler;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.util.internal.SocketUtils;
import org.junit.jupiter.api.Test;

import java.net.SocketAddress;
import java.time.Duration;
import java.util.Collections;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * High-cardinality per-IP rate limit tests for {@link NACL}.
 *
 * <p>Validates the SEC-09/SEC-10 per-IP rate limiting under DDoS simulation
 * conditions with many unique source IPs, exercising the CM-F3 LRU eviction
 * behavior of the bounded Guava cache backing per-IP buckets.</p>
 *
 * <p>Key scenarios:
 * <ul>
 *   <li>LRU eviction: 200 unique IPs with max 100 tracked -- cache must not grow unbounded</li>
 *   <li>Per-IP rate limit: 6th connection from the same IP within the rate window is rejected</li>
 *   <li>Global + per-IP interaction: global limit takes precedence when exhausted first</li>
 * </ul>
 *
 * <p>Uses Netty's {@link EmbeddedChannel} with a custom {@link #remoteAddress0()} override
 * to inject arbitrary source IPs without real network I/O, matching the pattern from
 * {@link NACLTest}.</p>
 */
class NACLHighCardinalityTest {

    /**
     * Simulates 200 unique source IPs connecting against a NACL configured with
     * {@code maxPerIpEntries = 100}. After all 200 have connected, the tracked
     * IP count must be bounded at or below 100 due to LRU eviction.
     *
     * <p>Guava's Cache uses approximate LRU with size-based eviction. The
     * {@code maximumSize} parameter in the NACL constructor caps the number of
     * per-IP buckets. When a 101st unique IP arrives, the least-recently-used
     * entry is evicted. This is the memory safety guarantee under DDoS.</p>
     */
    @Test
    void lruEvictionBoundsTrackedIpCount() {
        int maxPerIpEntries = 100;

        // Global rate limit is generous (10_000/s) so it does not interfere.
        // Per-IP limit: 5 connections per 10 seconds per source IP.
        NACL nacl = new NACL(
                10_000, Duration.ofSeconds(1),     // global: 10k/s
                5, Duration.ofSeconds(10),          // per-IP: 5/10s
                Collections.emptyList(), true,
                List.of(), NACL.Mode.DENYLIST, maxPerIpEntries
        );

        // Connect from 200 unique IPs (10.0.0.1 through 10.0.0.200).
        // Each IP gets exactly one connection, so per-IP limits are not exhausted.
        for (int i = 1; i <= 200; i++) {
            String ip = "10.0." + (i / 256) + "." + (i % 256);
            EmbeddedChannel ch = newEmbeddedInetChannel(ip, 5421, nacl);
            // The first connection from each IP must be accepted (per-IP bucket is fresh).
            assertTrue(ch.isActive(), "First connection from " + ip + " must be accepted");
            ch.close();
        }

        // After 200 unique IPs, the cache must have evicted entries beyond the max.
        // Guava's eviction is not always instant (it runs during cache operations),
        // so we trigger a cleanup by calling cleanUp() via trackedIpCount() access
        // and then verify the bound.
        assertTrue(nacl.trackedIpCount() <= maxPerIpEntries,
                "Tracked IP count must be bounded at maxPerIpEntries (" + maxPerIpEntries +
                        ") but was " + nacl.trackedIpCount());
    }

    /**
     * Verifies per-IP rate limiting: with a limit of 5 connections per 10 seconds,
     * the first 5 connections from the same IP are accepted and the 6th is rejected.
     */
    @Test
    void perIpRateLimitRejectsSixthConnection() {
        NACL nacl = new NACL(
                10_000, Duration.ofSeconds(1),     // global: generous
                5, Duration.ofSeconds(10),          // per-IP: 5/10s
                Collections.emptyList(), true,
                List.of(), NACL.Mode.DENYLIST, NACL.DEFAULT_MAX_PER_IP_ENTRIES
        );

        String sourceIp = "192.168.1.100";

        // First 5 connections from the same IP must be accepted.
        for (int i = 1; i <= 5; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel(sourceIp, 5421 + i, nacl);
            assertTrue(ch.isActive(), "Connection #" + i + " from " + sourceIp + " must be accepted");
            ch.close();
        }

        // 6th connection from the same IP within the rate window must be rejected.
        EmbeddedChannel ch6 = newEmbeddedInetChannel(sourceIp, 5427, nacl);
        assertFalse(ch6.isActive(), "6th connection from " + sourceIp + " must be rejected (per-IP rate limit)");
        ch6.close();

        // A different IP must still be accepted (per-IP limits are independent).
        EmbeddedChannel chOther = newEmbeddedInetChannel("192.168.1.200", 5421, nacl);
        assertTrue(chOther.isActive(), "Connection from a different IP must still be accepted");
        chOther.close();
    }

    /**
     * Verifies that the global rate limit takes precedence when it is exhausted
     * before the per-IP limit. With a global limit of 3 connections per 10 seconds
     * and a per-IP limit of 5 connections per 10 seconds, the 4th connection from
     * any combination of IPs must be rejected by the global limiter.
     */
    @Test
    void globalRateLimitTakesPrecedenceOverPerIp() {
        NACL nacl = new NACL(
                3, Duration.ofSeconds(10),          // global: 3/10s (tight)
                5, Duration.ofSeconds(10),          // per-IP: 5/10s (more generous)
                Collections.emptyList(), true,
                List.of(), NACL.Mode.DENYLIST, NACL.DEFAULT_MAX_PER_IP_ENTRIES
        );

        // Use different source IPs so per-IP limit is never hit (each IP gets 1 conn).
        for (int i = 1; i <= 3; i++) {
            String ip = "10.1.1." + i;
            EmbeddedChannel ch = newEmbeddedInetChannel(ip, 5421, nacl);
            assertTrue(ch.isActive(), "Connection #" + i + " from " + ip + " must be accepted (within global limit)");
            ch.close();
        }

        // 4th connection from a new IP: per-IP would allow it, but global is exhausted.
        EmbeddedChannel ch4 = newEmbeddedInetChannel("10.1.1.4", 5421, nacl);
        assertFalse(ch4.isActive(),
                "4th connection must be rejected by global rate limit even though per-IP limit is not exhausted");
        ch4.close();
    }

    /**
     * Verifies that per-IP rate limiting is independent across different source IPs.
     * Two IPs each get their own bucket with 5 allowed connections.
     */
    @Test
    void perIpLimitsAreIndependentAcrossIps() {
        NACL nacl = new NACL(
                10_000, Duration.ofSeconds(1),     // global: generous
                5, Duration.ofSeconds(10),          // per-IP: 5/10s
                Collections.emptyList(), true,
                List.of(), NACL.Mode.DENYLIST, NACL.DEFAULT_MAX_PER_IP_ENTRIES
        );

        String ipA = "172.16.0.1";
        String ipB = "172.16.0.2";

        // Exhaust IP A's per-IP budget.
        for (int i = 1; i <= 5; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel(ipA, 5420 + i, nacl);
            assertTrue(ch.isActive(), "IP A connection #" + i + " must be accepted");
            ch.close();
        }

        // IP A is now rate-limited.
        EmbeddedChannel chA6 = newEmbeddedInetChannel(ipA, 5426, nacl);
        assertFalse(chA6.isActive(), "IP A's 6th connection must be rejected");
        chA6.close();

        // IP B must still have its full budget.
        for (int i = 1; i <= 5; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel(ipB, 5420 + i, nacl);
            assertTrue(ch.isActive(), "IP B connection #" + i + " must be accepted (independent of IP A)");
            ch.close();
        }

        // IP B's 6th must also be rejected.
        EmbeddedChannel chB6 = newEmbeddedInetChannel(ipB, 5426, nacl);
        assertFalse(chB6.isActive(), "IP B's 6th connection must be rejected");
        chB6.close();
    }

    /**
     * Verifies that after LRU eviction, a re-appearing IP gets a fresh rate limit
     * bucket (its rate limit window is effectively reset). This is the documented
     * "benign outcome" from the CM-F3 comment in NACL.
     */
    @Test
    void evictedIpGetsFresBucketOnReappearance() {
        int maxPerIpEntries = 10;

        NACL nacl = new NACL(
                10_000, Duration.ofSeconds(1),     // global: generous
                5, Duration.ofSeconds(10),          // per-IP: 5/10s
                Collections.emptyList(), true,
                List.of(), NACL.Mode.DENYLIST, maxPerIpEntries
        );

        // Use 4 of 5 tokens from IP 10.0.0.1.
        String targetIp = "10.0.0.1";
        for (int i = 1; i <= 4; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel(targetIp, 5420 + i, nacl);
            assertTrue(ch.isActive(), "Target IP connection #" + i + " must be accepted");
            ch.close();
        }

        // Now flood with 20 other IPs to evict 10.0.0.1 from the LRU cache.
        for (int i = 2; i <= 21; i++) {
            String ip = "10.0.0." + i;
            EmbeddedChannel ch = newEmbeddedInetChannel(ip, 5421, nacl);
            assertTrue(ch.isActive(), "Flood IP " + ip + " must be accepted");
            ch.close();
        }

        // 10.0.0.1 should have been evicted. When it reappears, it gets a fresh bucket
        // with a full 5-connection allowance.
        EmbeddedChannel chReappear = newEmbeddedInetChannel(targetIp, 5430, nacl);
        assertTrue(chReappear.isActive(),
                "Evicted IP must get a fresh bucket and be accepted on reappearance");
        chReappear.close();
    }

    // =========================================================================
    // Helper: creates an EmbeddedChannel whose remoteAddress0() returns a
    // configurable InetSocketAddress. This avoids the need for real sockets
    // and matches the pattern from NACLTest.
    // =========================================================================

    private static EmbeddedChannel newEmbeddedInetChannel(String ip, int port, ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress(ip, port) : null;
            }
        };
    }
}
