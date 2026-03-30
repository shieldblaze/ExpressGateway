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
package com.shieldblaze.expressgateway.servicediscovery.server;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HealthAggregatorTest {

    private RegistrationStore store;
    private HealthAggregator aggregator;

    @BeforeEach
    void setup() {
        store = new RegistrationStore();
        aggregator = new HealthAggregator(store);
    }

    @Test
    void emptySummary() {
        var summary = aggregator.summarize();
        assertEquals(0, summary.total());
        assertEquals(0, summary.healthy());
        assertEquals(0, summary.unhealthy());
        assertEquals(0, summary.expired());
    }

    @Test
    void allHealthySummary() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        store.register(new Node("svc-2", "10.0.0.2", 8081, true), 60);

        var summary = aggregator.summarize();
        assertEquals(2, summary.total());
        assertEquals(2, summary.healthy());
        assertEquals(0, summary.unhealthy());
    }

    @Test
    void mixedHealthSummary() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        store.register(new Node("svc-2", "10.0.0.2", 8081, true), 60);
        aggregator.markUnhealthy("svc-2");

        var summary = aggregator.summarize();
        assertEquals(2, summary.total());
        assertEquals(1, summary.healthy());
        assertEquals(1, summary.unhealthy());
    }

    @Test
    void expiredSummary() throws InterruptedException {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 1); // 1s TTL
        store.register(new Node("svc-2", "10.0.0.2", 8081, true), 0); // no TTL

        Thread.sleep(1200);

        var summary = aggregator.summarize();
        assertEquals(2, summary.total());
        assertEquals(1, summary.healthy()); // svc-2
        assertEquals(1, summary.expired()); // svc-1
    }

    @Test
    void processHeartbeat() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        assertTrue(aggregator.processHeartbeat("svc-1"));
    }

    @Test
    void processHeartbeatUnknownNode() {
        assertFalse(aggregator.processHeartbeat("unknown"));
    }

    @Test
    void markAndVerifyUnhealthy() {
        store.register(new Node("svc-1", "10.0.0.1", 8080, false), 60);
        aggregator.markUnhealthy("svc-1");
        assertFalse(store.get("svc-1").get().healthy());
    }
}
