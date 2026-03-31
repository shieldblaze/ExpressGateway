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

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RegistrationEntryTest {

    @Test
    void createEntry() {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = RegistrationEntry.create(node, 60);

        assertNotNull(entry.registeredAt());
        assertNotNull(entry.lastHeartbeat());
        assertTrue(entry.healthy());
        assertFalse(entry.isExpired());
    }

    @Test
    void withHeartbeat() throws InterruptedException {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = RegistrationEntry.create(node, 60);
        var original = entry.lastHeartbeat();

        Thread.sleep(10);
        RegistrationEntry updated = entry.withHeartbeat();
        assertTrue(updated.lastHeartbeat().isAfter(original));
        assertTrue(updated.healthy());
    }

    @Test
    void asUnhealthy() {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = RegistrationEntry.create(node, 60);
        assertTrue(entry.healthy());

        RegistrationEntry unhealthy = entry.asUnhealthy();
        assertFalse(unhealthy.healthy());
    }

    @Test
    void expirationWithTtl() throws InterruptedException {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = RegistrationEntry.create(node, 1); // 1 second TTL
        assertFalse(entry.isExpired());

        Thread.sleep(1200);
        assertTrue(entry.isExpired());
    }

    @Test
    void noExpirationWithZeroTtl() throws InterruptedException {
        Node node = new Node("svc-1", "10.0.0.1", 8080, false);
        RegistrationEntry entry = RegistrationEntry.create(node, 0);
        assertFalse(entry.isExpired());

        Thread.sleep(100);
        assertFalse(entry.isExpired()); // 0 means no expiry
    }
}
