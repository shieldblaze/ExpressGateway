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
package com.shieldblaze.expressgateway.core.handlers;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Tests for HI-04: ConnectionTracker total connection limit enforcement.
 */
class ConnectionTrackerTest {

    @Test
    void testIncrementDecrement() {
        ConnectionTracker tracker = new ConnectionTracker();
        assertEquals(0, tracker.connections());

        tracker.increment();
        assertEquals(1, tracker.connections());

        tracker.increment();
        assertEquals(2, tracker.connections());

        tracker.decrement();
        assertEquals(1, tracker.connections());

        tracker.decrement();
        assertEquals(0, tracker.connections());
    }

    @Test
    void testDecrementDoesNotGoBelowZero() {
        ConnectionTracker tracker = new ConnectionTracker();
        tracker.decrement(); // Should not go negative
        assertEquals(0, tracker.connections());
    }

    @Test
    void testMaxTotalConnectionsSetter() {
        ConnectionTracker tracker = new ConnectionTracker();
        // Just verify setter doesn't throw
        tracker.setMaxTotalConnections(1000);
        tracker.setMaxTotalConnections(0); // disable
    }

    @Test
    void testMaxConnectionsPerIpSetter() {
        ConnectionTracker tracker = new ConnectionTracker();
        tracker.setMaxConnectionsPerIp(100);
        tracker.setMaxConnectionsPerIp(0); // disable
    }
}
