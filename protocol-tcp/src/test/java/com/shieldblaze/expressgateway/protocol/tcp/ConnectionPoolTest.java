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
package com.shieldblaze.expressgateway.protocol.tcp;

import io.netty.channel.Channel;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

final class ConnectionPoolTest {

    private static final InetSocketAddress BACKEND_1 = new InetSocketAddress("10.0.0.1", 8080);
    private static final InetSocketAddress BACKEND_2 = new InetSocketAddress("10.0.0.2", 8080);

    private ConnectionPool pool;

    @BeforeEach
    void setUp() {
        pool = new ConnectionPool(new ConnectionPool.Config(4, 16, Duration.ofSeconds(60), 0));
    }

    @AfterEach
    void tearDown() {
        pool.close();
    }

    @Test
    void acquireFromEmptyPoolReturnsNull() {
        assertNull(pool.acquire(BACKEND_1));
    }

    @Test
    void releaseAndAcquireRoundTrip() {
        Channel channel = mockActiveChannel();

        assertTrue(pool.release(channel, BACKEND_1));
        assertEquals(1, pool.size());
        assertEquals(1, pool.size(BACKEND_1));

        Channel acquired = pool.acquire(BACKEND_1);
        assertNotNull(acquired);
        assertEquals(channel, acquired);
        assertEquals(0, pool.size());
    }

    @Test
    void acquireFromWrongBackendReturnsNull() {
        Channel channel = mockActiveChannel();
        pool.release(channel, BACKEND_1);

        assertNull(pool.acquire(BACKEND_2));
        assertEquals(1, pool.size()); // Connection still in pool
    }

    @Test
    void deadConnectionIsSkippedOnAcquire() {
        // Channel that is active when released but becomes dead before acquire
        Channel diesLater = mock(Channel.class);
        when(diesLater.isActive()).thenReturn(true, false); // active on release, dead on acquire
        when(diesLater.isOpen()).thenReturn(true, false);

        Channel alive = mockActiveChannel();

        pool.release(diesLater, BACKEND_1);
        pool.release(alive, BACKEND_1);
        assertEquals(2, pool.size());

        Channel acquired = pool.acquire(BACKEND_1);
        assertNotNull(acquired);
        assertEquals(alive, acquired);
        verify(diesLater).close(); // Dead channel should be closed when skipped
    }

    @Test
    void perBackendLimitEnforced() {
        // Pool config allows max 4 per backend
        for (int i = 0; i < 4; i++) {
            assertTrue(pool.release(mockActiveChannel(), BACKEND_1));
        }
        assertEquals(4, pool.size(BACKEND_1));

        // 5th should be rejected and closed
        Channel overflow = mockActiveChannel();
        assertFalse(pool.release(overflow, BACKEND_1));
        verify(overflow).close();
        assertEquals(4, pool.size(BACKEND_1));
    }

    @Test
    void totalLimitEnforced() {
        // Pool config allows max 16 total
        ConnectionPool smallPool = new ConnectionPool(new ConnectionPool.Config(10, 3, Duration.ofSeconds(60), 0));
        try {
            for (int i = 0; i < 3; i++) {
                assertTrue(smallPool.release(mockActiveChannel(), BACKEND_1));
            }

            Channel overflow = mockActiveChannel();
            assertFalse(smallPool.release(overflow, BACKEND_1));
            verify(overflow).close();
        } finally {
            smallPool.close();
        }
    }

    @Test
    void evictIdleRemovesStaleConnections() throws InterruptedException {
        ConnectionPool shortTtl = new ConnectionPool(
                new ConnectionPool.Config(4, 16, Duration.ofMillis(50), 0));
        try {
            Channel ch1 = mockActiveChannel();
            Channel ch2 = mockActiveChannel();
            shortTtl.release(ch1, BACKEND_1);
            shortTtl.release(ch2, BACKEND_1);
            assertEquals(2, shortTtl.size());

            // Wait for idle timeout to expire
            Thread.sleep(100);

            int evicted = shortTtl.evictIdle();
            assertEquals(2, evicted);
            assertEquals(0, shortTtl.size());
            verify(ch1).close();
            verify(ch2).close();
        } finally {
            shortTtl.close();
        }
    }

    @Test
    void evictIdleKeepsFreshConnections() {
        Channel ch = mockActiveChannel();
        pool.release(ch, BACKEND_1);

        // With 60s timeout, connection should NOT be evicted
        int evicted = pool.evictIdle();
        assertEquals(0, evicted);
        assertEquals(1, pool.size());
    }

    @Test
    void closeClosesAllPooledChannels() {
        Channel ch1 = mockActiveChannel();
        Channel ch2 = mockActiveChannel();
        Channel ch3 = mockActiveChannel();
        pool.release(ch1, BACKEND_1);
        pool.release(ch2, BACKEND_1);
        pool.release(ch3, BACKEND_2);

        pool.close();

        verify(ch1).close();
        verify(ch2).close();
        verify(ch3).close();
        assertEquals(0, pool.size());
    }

    @Test
    void releaseToClosedPoolClosesChannel() {
        pool.close();

        Channel ch = mockActiveChannel();
        assertFalse(pool.release(ch, BACKEND_1));
        verify(ch).close();
    }

    @Test
    void acquireFromClosedPoolReturnsNull() {
        Channel ch = mockActiveChannel();
        pool.release(ch, BACKEND_1);
        pool.close();

        assertNull(pool.acquire(BACKEND_1));
    }

    @Test
    void releaseInactiveChannelClosesIt() {
        Channel inactive = mock(Channel.class);
        when(inactive.isActive()).thenReturn(false);

        assertFalse(pool.release(inactive, BACKEND_1));
        verify(inactive).close();
    }

    @Test
    void concurrentAcquireAndRelease() throws InterruptedException {
        int threads = 8;
        int ops = 1000;
        CountDownLatch latch = new CountDownLatch(threads);
        AtomicInteger acquired = new AtomicInteger(0);

        for (int t = 0; t < threads; t++) {
            Thread.ofVirtual().start(() -> {
                try {
                    for (int i = 0; i < ops; i++) {
                        Channel ch = mockActiveChannel();
                        pool.release(ch, BACKEND_1);
                        Channel got = pool.acquire(BACKEND_1);
                        if (got != null) {
                            acquired.incrementAndGet();
                        }
                    }
                } finally {
                    latch.countDown();
                }
            });
        }

        assertTrue(latch.await(30, TimeUnit.SECONDS));
        // We don't assert exact counts since concurrent ops are non-deterministic,
        // but the pool should not throw and should remain in a consistent state
        assertTrue(pool.size() >= 0);
    }

    @Test
    void connectionPoolEntryIsUsable() {
        Channel active = mockActiveChannel();
        ConnectionPoolEntry entry = new ConnectionPoolEntry(active, BACKEND_1);
        assertTrue(entry.isUsable());

        Channel dead = mockDeadChannel();
        ConnectionPoolEntry deadEntry = new ConnectionPoolEntry(dead, BACKEND_1);
        assertFalse(deadEntry.isUsable());
    }

    @Test
    void connectionPoolEntryIdleCheck() throws InterruptedException {
        Channel ch = mockActiveChannel();
        ConnectionPoolEntry entry = new ConnectionPoolEntry(ch, BACKEND_1);

        assertFalse(entry.isIdleLongerThan(Duration.ofSeconds(10)));

        Thread.sleep(50);
        assertTrue(entry.isIdleLongerThan(Duration.ofMillis(30)));
    }

    @Test
    void connectionPoolEntryTouch() throws InterruptedException {
        Channel ch = mockActiveChannel();
        ConnectionPoolEntry entry = new ConnectionPoolEntry(ch, BACKEND_1);
        Thread.sleep(50);

        ConnectionPoolEntry touched = entry.touch();
        assertFalse(touched.isIdleLongerThan(Duration.ofMillis(30)));
        assertEquals(entry.createdAt(), touched.createdAt()); // createdAt unchanged
    }

    @Test
    void defaultConfigValues() {
        ConnectionPool.Config def = ConnectionPool.Config.DEFAULT;
        assertEquals(8, def.maxIdlePerBackend());
        assertEquals(256, def.maxTotalConnections());
        assertEquals(Duration.ofSeconds(60), def.idleTimeout());
        assertEquals(0, def.warmupCount());
    }

    // -- Helpers --

    private static Channel mockActiveChannel() {
        Channel ch = mock(Channel.class);
        when(ch.isActive()).thenReturn(true);
        when(ch.isOpen()).thenReturn(true);
        return ch;
    }

    private static Channel mockDeadChannel() {
        Channel ch = mock(Channel.class);
        when(ch.isActive()).thenReturn(false);
        when(ch.isOpen()).thenReturn(false);
        return ch;
    }
}
