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

import java.net.InetSocketAddress;
import java.time.Instant;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Tests for the sealed UdpProxyEvent hierarchy.
 */
final class UdpProxyEventTest {

    private static final InetSocketAddress CLIENT_ADDR = new InetSocketAddress("10.0.0.1", 12345);
    private static final InetSocketAddress BACKEND_ADDR = new InetSocketAddress("10.0.0.2", 9090);

    @Test
    void sessionCreatedEvent() {
        var event = new UdpProxyEvent.SessionCreated(Instant.now(), CLIENT_ADDR, BACKEND_ADDR);
        assertInstanceOf(UdpProxyEvent.class, event);
        assertEquals(CLIENT_ADDR, event.clientAddress());
        assertEquals(BACKEND_ADDR, event.backendAddress());
        assertNotNull(event.timestamp());
    }

    @Test
    void sessionExpiredEvent() {
        var event = new UdpProxyEvent.SessionExpired(Instant.now(), CLIENT_ADDR, BACKEND_ADDR, 42);
        assertEquals(42, event.packetCount());
    }

    @Test
    void datagramDroppedEvent() {
        var event = new UdpProxyEvent.DatagramDropped(
                Instant.now(), CLIENT_ADDR, UdpProxyEvent.DropReason.RATE_LIMITED);
        assertEquals(UdpProxyEvent.DropReason.RATE_LIMITED, event.reason());
    }

    @Test
    void deliveryFailedEvent() {
        var cause = new RuntimeException("backend down");
        var event = new UdpProxyEvent.DeliveryFailed(Instant.now(), CLIENT_ADDR, cause);
        assertEquals(cause, event.cause());
    }

    /**
     * Exhaustive pattern matching -- compile-time safety that all subtypes are covered.
     */
    @Test
    void exhaustivePatternMatch() {
        UdpProxyEvent event = new UdpProxyEvent.SessionCreated(Instant.now(), CLIENT_ADDR, BACKEND_ADDR);

        String desc = switch (event) {
            case UdpProxyEvent.SessionCreated e -> "created " + e.clientAddress();
            case UdpProxyEvent.SessionExpired e -> "expired " + e.packetCount();
            case UdpProxyEvent.DatagramDropped e -> "dropped " + e.reason();
            case UdpProxyEvent.DeliveryFailed e -> "failed " + e.cause();
        };

        assertNotNull(desc);
        assertEquals("created " + CLIENT_ADDR, desc);
    }

    @Test
    void allDropReasonsAccessible() {
        assertEquals(4, UdpProxyEvent.DropReason.values().length);
        assertNotNull(UdpProxyEvent.DropReason.RATE_LIMITED);
        assertNotNull(UdpProxyEvent.DropReason.NO_BACKEND);
        assertNotNull(UdpProxyEvent.DropReason.OVERSIZED);
        assertNotNull(UdpProxyEvent.DropReason.NOT_READY);
    }
}
