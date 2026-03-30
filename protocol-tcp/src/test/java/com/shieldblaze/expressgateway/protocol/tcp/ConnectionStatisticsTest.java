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

import org.junit.jupiter.api.Test;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class ConnectionStatisticsTest {

    @Test
    void recordBytesAndMessages() {
        ConnectionStatistics stats = new ConnectionStatistics();

        stats.recordBytesRead(100);
        stats.recordBytesRead(200);
        stats.recordBytesWritten(50);

        ConnectionStatistics.Snapshot snap = stats.snapshot();
        assertEquals(300, snap.bytesRead());
        assertEquals(50, snap.bytesWritten());
        assertEquals(2, snap.messagesRead());
        assertEquals(1, snap.messagesWritten());
        assertEquals(350, snap.totalBytes());
    }

    @Test
    void backpressurePauseCounter() {
        ConnectionStatistics stats = new ConnectionStatistics();

        stats.recordBackpressurePause();
        stats.recordBackpressurePause();
        stats.recordBackpressurePause();

        assertEquals(3, stats.snapshot().backpressurePauses());
    }

    @Test
    void idleNanosIsPositiveAfterDelay() throws InterruptedException {
        ConnectionStatistics stats = new ConnectionStatistics();
        stats.recordBytesRead(1);
        Thread.sleep(50);

        ConnectionStatistics.Snapshot snap = stats.snapshot();
        assertTrue(snap.idleNanos() >= 40_000_000L,
                "idleNanos should be at least 40ms, got " + snap.idleNanos());
    }

    @Test
    void uptimeMillisGrowsOverTime() throws InterruptedException {
        ConnectionStatistics stats = new ConnectionStatistics();
        Thread.sleep(100);
        ConnectionStatistics.Snapshot snap = stats.snapshot();
        assertTrue(snap.uptimeMillis() >= 90,
                "uptimeMillis should be at least 90ms, got " + snap.uptimeMillis());
    }

    @Test
    void concurrentUpdatesDoNotLoseData() throws InterruptedException {
        ConnectionStatistics stats = new ConnectionStatistics();
        int threads = 8;
        int iterations = 10_000;
        CountDownLatch latch = new CountDownLatch(threads);

        for (int t = 0; t < threads; t++) {
            Thread.ofVirtual().start(() -> {
                for (int i = 0; i < iterations; i++) {
                    stats.recordBytesRead(1);
                    stats.recordBytesWritten(1);
                    stats.recordBackpressurePause();
                }
                latch.countDown();
            });
        }

        assertTrue(latch.await(10, TimeUnit.SECONDS));

        ConnectionStatistics.Snapshot snap = stats.snapshot();
        long expectedMessages = (long) threads * iterations;
        assertEquals(expectedMessages, snap.messagesRead());
        assertEquals(expectedMessages, snap.messagesWritten());
        assertEquals(expectedMessages, snap.bytesRead());
        assertEquals(expectedMessages, snap.bytesWritten());
        assertEquals(expectedMessages, snap.backpressurePauses());
    }
}
