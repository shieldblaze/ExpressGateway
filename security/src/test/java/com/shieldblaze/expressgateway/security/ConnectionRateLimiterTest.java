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

import static org.junit.jupiter.api.Assertions.*;

class ConnectionRateLimiterTest {

    @Test
    void acceptsWithinLimit() {
        ConnectionRateLimiter limiter = new ConnectionRateLimiter(5, Duration.ofSeconds(10));

        for (int i = 0; i < 5; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel("127.0.0.1", 5000 + i, limiter);
            assertTrue(ch.isActive(), "Connection #" + (i + 1) + " should be accepted");
            ch.close();
        }

        assertEquals(5, limiter.totalAccepted());
        assertEquals(0, limiter.totalRejected());
    }

    @Test
    void rejectsOverLimit() {
        ConnectionRateLimiter limiter = new ConnectionRateLimiter(3, Duration.ofSeconds(10));

        // Accept first 3
        for (int i = 0; i < 3; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel("127.0.0.1", 5000 + i, limiter);
            assertTrue(ch.isActive(), "Connection #" + (i + 1) + " should be accepted");
            ch.close();
        }

        // 4th should be rejected
        EmbeddedChannel ch4 = newEmbeddedInetChannel("127.0.0.1", 5003, limiter);
        assertFalse(ch4.isActive(), "4th connection should be rejected");
        ch4.close();

        assertEquals(3, limiter.totalAccepted());
        assertEquals(1, limiter.totalRejected());
    }

    @Test
    void perIpIndependence() {
        ConnectionRateLimiter limiter = new ConnectionRateLimiter(2, Duration.ofSeconds(10));

        // 2 connections from IP A
        for (int i = 0; i < 2; i++) {
            EmbeddedChannel ch = newEmbeddedInetChannel("10.0.0.1", 5000 + i, limiter);
            assertTrue(ch.isActive());
            ch.close();
        }

        // 3rd from IP A should fail
        EmbeddedChannel chA3 = newEmbeddedInetChannel("10.0.0.1", 5002, limiter);
        assertFalse(chA3.isActive(), "IP A's 3rd connection should fail");
        chA3.close();

        // IP B should still have its budget
        EmbeddedChannel chB1 = newEmbeddedInetChannel("10.0.0.2", 5000, limiter);
        assertTrue(chB1.isActive(), "IP B's 1st connection should succeed");
        chB1.close();
    }

    @Test
    void constructorValidation() {
        assertThrows(IllegalArgumentException.class,
                () -> new ConnectionRateLimiter(0, Duration.ofSeconds(1)));
        assertThrows(NullPointerException.class,
                () -> new ConnectionRateLimiter(5, null));
        assertThrows(IllegalArgumentException.class,
                () -> new ConnectionRateLimiter(5, Duration.ZERO));
    }

    @Test
    void slidingWindowCounterBasic() {
        ConnectionRateLimiter.SlidingWindowCounter counter =
                new ConnectionRateLimiter.SlidingWindowCounter(3);

        long now = System.nanoTime();
        long windowNanos = Duration.ofSeconds(10).toNanos();

        assertTrue(counter.tryAcquire(now, windowNanos));
        assertTrue(counter.tryAcquire(now + 1, windowNanos));
        assertTrue(counter.tryAcquire(now + 2, windowNanos));
        assertFalse(counter.tryAcquire(now + 3, windowNanos), "4th acquire should fail");

        assertEquals(3, counter.currentCount());
    }

    @Test
    void slidingWindowCounterEviction() {
        ConnectionRateLimiter.SlidingWindowCounter counter =
                new ConnectionRateLimiter.SlidingWindowCounter(3);

        long windowNanos = Duration.ofSeconds(1).toNanos();
        long now = System.nanoTime();

        // Fill the window
        assertTrue(counter.tryAcquire(now, windowNanos));
        assertTrue(counter.tryAcquire(now + 1, windowNanos));
        assertTrue(counter.tryAcquire(now + 2, windowNanos));
        assertFalse(counter.tryAcquire(now + 3, windowNanos));

        // After the window elapses, old entries should be evicted
        long later = now + windowNanos + 1;
        assertTrue(counter.tryAcquire(later, windowNanos),
                "Should succeed after old entries expire");
    }

    private static EmbeddedChannel newEmbeddedInetChannel(String ip, int port, ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress(ip, port) : null;
            }
        };
    }
}
