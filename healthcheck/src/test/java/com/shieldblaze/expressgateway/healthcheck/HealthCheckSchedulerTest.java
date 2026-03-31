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
package com.shieldblaze.expressgateway.healthcheck;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.*;

class HealthCheckSchedulerTest {

    private static final InetSocketAddress ADDR = new InetSocketAddress("127.0.0.1", 1);
    private static final Duration TIMEOUT = Duration.ofSeconds(1);

    static class CountingHealthCheck extends HealthCheck {
        final AtomicInteger runCount = new AtomicInteger();

        CountingHealthCheck() {
            super(ADDR, TIMEOUT, 10);
        }

        @Override
        public void run() {
            runCount.incrementAndGet();
            markSuccess();
        }
    }

    @Test
    void registerAndStart() throws InterruptedException {
        CountingHealthCheck hc = new CountingHealthCheck();

        try (HealthCheckScheduler scheduler = new HealthCheckScheduler()) {
            scheduler.register(hc, Duration.ofSeconds(1));
            assertEquals(1, scheduler.checks().size());

            scheduler.start();
            assertTrue(scheduler.isRunning());

            // Wait for at least one tick
            Thread.sleep(2000);

            assertTrue(hc.runCount.get() > 0,
                    "Health check should have been executed at least once");
        }
    }

    @Test
    void unregister() {
        CountingHealthCheck hc = new CountingHealthCheck();

        try (HealthCheckScheduler scheduler = new HealthCheckScheduler()) {
            scheduler.register(hc, Duration.ofSeconds(1));
            assertEquals(1, scheduler.checks().size());

            scheduler.unregister(hc);
            assertEquals(0, scheduler.checks().size());
        }
    }

    @Test
    void closeStopsScheduler() {
        HealthCheckScheduler scheduler = new HealthCheckScheduler();
        scheduler.start();
        assertTrue(scheduler.isRunning());

        scheduler.close();
        assertFalse(scheduler.isRunning());
    }

    @Test
    void multipleChecksExecuteConcurrently() throws InterruptedException {
        CountingHealthCheck hc1 = new CountingHealthCheck();
        CountingHealthCheck hc2 = new CountingHealthCheck();

        try (HealthCheckScheduler scheduler = new HealthCheckScheduler()) {
            scheduler.register(hc1, Duration.ofSeconds(1));
            scheduler.register(hc2, Duration.ofSeconds(1));

            scheduler.start();
            Thread.sleep(2000);

            assertTrue(hc1.runCount.get() > 0, "First check should have run");
            assertTrue(hc2.runCount.get() > 0, "Second check should have run");
        }
    }
}
