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
package com.shieldblaze.expressgateway.servicediscovery.client;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ServiceEntryTest {

    @Test
    void createWithFactoryMethod() {
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, true);
        assertEquals("svc-1", entry.id());
        assertEquals("10.0.0.1", entry.ipAddress());
        assertEquals(8080, entry.port());
        assertTrue(entry.tlsEnabled());
        assertTrue(entry.healthy());
        assertNotNull(entry.fetchedAt());
    }

    @Test
    void address() {
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        assertEquals("10.0.0.1:8080", entry.address());
    }

    @Test
    void freshness() throws InterruptedException {
        ServiceEntry entry = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        assertTrue(entry.isFresh(60_000));

        // With a tiny TTL, it should expire quickly
        Thread.sleep(50);
        assertFalse(entry.isFresh(10));
    }

    @Test
    void recordEquality() {
        ServiceEntry e1 = ServiceEntry.of("svc-1", "10.0.0.1", 8080, false);
        ServiceEntry e2 = new ServiceEntry("svc-1", "10.0.0.1", 8080, false, true, e1.fetchedAt());
        assertEquals(e1, e2);
    }
}
