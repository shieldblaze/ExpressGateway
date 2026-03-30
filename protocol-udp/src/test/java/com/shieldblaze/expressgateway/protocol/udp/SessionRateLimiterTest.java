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

import org.junit.jupiter.api.Test;

import java.net.InetAddress;
import java.net.UnknownHostException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for the per-source UDP rate limiter.
 */
final class SessionRateLimiterTest {

    private static InetAddress addr(String ip) {
        try {
            return InetAddress.getByName(ip);
        } catch (UnknownHostException e) {
            throw new RuntimeException(e);
        }
    }

    @Test
    void disabledLimiterAllowsAllTraffic() {
        SessionRateLimiter limiter = new SessionRateLimiter(SessionRateLimiter.Config.DISABLED);
        InetAddress src = addr("10.0.0.1");

        for (int i = 0; i < 100_000; i++) {
            assertTrue(limiter.tryAcquire(src));
        }
        assertEquals(0, limiter.totalDropped());
    }

    @Test
    void burstAllowedUpToCapacity() {
        // 100 pps with burst of 10
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(100, 10));
        InetAddress src = addr("10.0.0.1");

        // Initial burst of 10 should all pass
        for (int i = 0; i < 10; i++) {
            assertTrue(limiter.tryAcquire(src), "Packet " + i + " should be allowed");
        }

        // 11th should be rejected (no time to refill)
        assertFalse(limiter.tryAcquire(src));
        assertEquals(1, limiter.totalDropped());
    }

    @Test
    void tokensRefillOverTime() throws InterruptedException {
        // 1000 pps with burst of 5
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(1000, 5));
        InetAddress src = addr("10.0.0.1");

        // Drain all tokens
        for (int i = 0; i < 5; i++) {
            assertTrue(limiter.tryAcquire(src));
        }
        assertFalse(limiter.tryAcquire(src));

        // Wait 10ms -- at 1000 pps, ~10 tokens should refill
        Thread.sleep(20);

        // Should be able to acquire again
        assertTrue(limiter.tryAcquire(src),
                "Tokens should have refilled after waiting");
    }

    @Test
    void differentSourcesAreIndependent() {
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(100, 5));
        InetAddress src1 = addr("10.0.0.1");
        InetAddress src2 = addr("10.0.0.2");

        // Drain src1
        for (int i = 0; i < 5; i++) {
            assertTrue(limiter.tryAcquire(src1));
        }
        assertFalse(limiter.tryAcquire(src1));

        // src2 should still have full capacity
        for (int i = 0; i < 5; i++) {
            assertTrue(limiter.tryAcquire(src2));
        }

        assertEquals(2, limiter.trackedSources());
    }

    @Test
    void evictStaleBuckets() throws InterruptedException {
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(1000, 10));
        InetAddress src1 = addr("10.0.0.1");
        InetAddress src2 = addr("10.0.0.2");

        limiter.tryAcquire(src1);
        limiter.tryAcquire(src2);
        assertEquals(2, limiter.trackedSources());

        // Wait then evict
        Thread.sleep(100);
        int evicted = limiter.evictStale(50_000_000L); // 50ms threshold
        assertEquals(2, evicted);
        assertEquals(0, limiter.trackedSources());
    }

    @Test
    void evictStaleKeepsFreshBuckets() {
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(1000, 10));
        InetAddress src = addr("10.0.0.1");

        limiter.tryAcquire(src);
        int evicted = limiter.evictStale(60_000_000_000L); // 60s threshold
        assertEquals(0, evicted);
        assertEquals(1, limiter.trackedSources());
    }

    @Test
    void concurrentAccessDoesNotLoseData() throws InterruptedException {
        SessionRateLimiter limiter = new SessionRateLimiter(
                new SessionRateLimiter.Config(1_000_000, 100));
        InetAddress src = addr("10.0.0.1");
        int threads = 8;
        int opsPerThread = 1000;
        CountDownLatch latch = new CountDownLatch(threads);
        AtomicInteger allowed = new AtomicInteger(0);
        AtomicInteger denied = new AtomicInteger(0);

        for (int t = 0; t < threads; t++) {
            Thread.ofVirtual().start(() -> {
                for (int i = 0; i < opsPerThread; i++) {
                    if (limiter.tryAcquire(src)) {
                        allowed.incrementAndGet();
                    } else {
                        denied.incrementAndGet();
                    }
                }
                latch.countDown();
            });
        }

        assertTrue(latch.await(10, TimeUnit.SECONDS));
        assertEquals(threads * opsPerThread, allowed.get() + denied.get(),
                "Total ops should equal allowed + denied");
        assertEquals(denied.get(), limiter.totalDropped(),
                "totalDropped should match denied count");
    }

    @Test
    void configValidation() {
        var config = new SessionRateLimiter.Config(10_000, 1_000);
        assertEquals(10_000, config.packetsPerSecond());
        assertEquals(1_000, config.burstSize());
        assertFalse(config.isDisabled());
        assertTrue(SessionRateLimiter.Config.DISABLED.isDisabled());
    }

    @Test
    void defaultConfig() {
        SessionRateLimiter.Config def = SessionRateLimiter.Config.DEFAULT;
        assertEquals(10_000, def.packetsPerSecond());
        assertEquals(1_000, def.burstSize());
    }
}
