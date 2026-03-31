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

import java.net.InetSocketAddress;
import java.time.Instant;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Tests for the sealed ProxyEvent hierarchy.
 * Validates exhaustive pattern matching and record construction.
 */
final class ProxyEventTest {

    private static final InetSocketAddress CLIENT_ADDR = new InetSocketAddress("10.0.0.1", 12345);
    private static final InetSocketAddress BACKEND_ADDR = new InetSocketAddress("10.0.0.2", 8080);

    @Test
    void clientConnectedEventConstruction() {
        var event = new ProxyEvent.ClientConnected(Instant.now(), CLIENT_ADDR);
        assertInstanceOf(ProxyEvent.class, event);
        assertEquals(CLIENT_ADDR, event.clientAddress());
        assertNotNull(event.timestamp());
    }

    @Test
    void backendConnectedEventConstruction() {
        var event = new ProxyEvent.BackendConnected(Instant.now(), CLIENT_ADDR, BACKEND_ADDR, 15);
        assertEquals(15, event.connectTimeMs());
        assertEquals(BACKEND_ADDR, event.backendAddress());
    }

    @Test
    void backendConnectFailedEventConstruction() {
        var cause = new RuntimeException("connection refused");
        var event = new ProxyEvent.BackendConnectFailed(Instant.now(), CLIENT_ADDR, BACKEND_ADDR, cause);
        assertEquals(cause, event.cause());
    }

    @Test
    void backpressureEventsConstruction() {
        var engaged = new ProxyEvent.BackpressureEngaged(Instant.now(), ProxyEvent.Direction.CLIENT_TO_BACKEND);
        assertEquals(ProxyEvent.Direction.CLIENT_TO_BACKEND, engaged.direction());

        var released = new ProxyEvent.BackpressureReleased(Instant.now(), ProxyEvent.Direction.BACKEND_TO_CLIENT);
        assertEquals(ProxyEvent.Direction.BACKEND_TO_CLIENT, released.direction());
    }

    @Test
    void halfCloseEventConstruction() {
        var event = new ProxyEvent.HalfClose(Instant.now(), ProxyEvent.Direction.CLIENT_TO_BACKEND);
        assertInstanceOf(ProxyEvent.class, event);
        assertEquals(ProxyEvent.Direction.CLIENT_TO_BACKEND, event.direction());
    }

    @Test
    void sessionClosedEventConstruction() {
        ConnectionStatistics stats = new ConnectionStatistics();
        stats.recordBytesRead(1000);
        stats.recordBytesWritten(500);

        var event = new ProxyEvent.SessionClosed(
                Instant.now(),
                stats.snapshot(),
                ProxyEvent.CloseReason.CLIENT_CLOSED
        );

        assertEquals(ProxyEvent.CloseReason.CLIENT_CLOSED, event.reason());
        assertEquals(1000, event.statistics().bytesRead());
        assertEquals(500, event.statistics().bytesWritten());
    }

    /**
     * Exhaustive pattern matching with sealed interface -- compile-time verification
     * that all subtypes are covered. Adding a new ProxyEvent record without updating
     * this switch will cause a compilation error.
     */
    @Test
    void exhaustivePatternMatch() {
        ProxyEvent event = new ProxyEvent.ClientConnected(Instant.now(), CLIENT_ADDR);

        String description = switch (event) {
            case ProxyEvent.ClientConnected e -> "client " + e.clientAddress();
            case ProxyEvent.BackendConnected e -> "backend " + e.backendAddress();
            case ProxyEvent.BackendConnectFailed e -> "failed " + e.cause();
            case ProxyEvent.BackpressureEngaged e -> "bp engaged " + e.direction();
            case ProxyEvent.BackpressureReleased e -> "bp released " + e.direction();
            case ProxyEvent.HalfClose e -> "half-close " + e.direction();
            case ProxyEvent.SessionClosed e -> "closed " + e.reason();
        };

        assertNotNull(description);
        assertEquals("client " + CLIENT_ADDR, description);
    }

    @Test
    void allCloseReasonsAccessible() {
        for (ProxyEvent.CloseReason reason : ProxyEvent.CloseReason.values()) {
            assertNotNull(reason.name());
        }
        assertEquals(7, ProxyEvent.CloseReason.values().length);
    }

    @Test
    void allDirectionsAccessible() {
        assertEquals(2, ProxyEvent.Direction.values().length);
        assertNotNull(ProxyEvent.Direction.CLIENT_TO_BACKEND);
        assertNotNull(ProxyEvent.Direction.BACKEND_TO_CLIENT);
    }
}
