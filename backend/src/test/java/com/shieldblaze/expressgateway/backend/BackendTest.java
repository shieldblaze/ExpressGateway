/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class BackendTest {

    @Test
    void testCreate() {
        Backend backend = new Backend(new InetSocketAddress("10.1.1.1", 9110));
        assertEquals("10.1.1.1", backend.getSocketAddress().getAddress().getHostAddress());
        assertEquals(9110, backend.getSocketAddress().getPort());


        backend = new Backend(new InetSocketAddress("10.1.1.1", 9110), 10, 100);
        assertEquals("10.1.1.1", backend.getSocketAddress().getAddress().getHostAddress());
        assertEquals(9110, backend.getSocketAddress().getPort());
        assertEquals(10, backend.getWeight());
        assertEquals(100, backend.getMaxConnections());
    }

    @Test
    void throwException() {
        assertThrows(IllegalArgumentException.class, () -> new Backend(new InetSocketAddress("10.1.1.1", 9110), 0, 100));
        assertDoesNotThrow(() -> new Backend(new InetSocketAddress("10.1.1.1", 9110), 1, 100));
        assertThrows(IllegalArgumentException.class, () -> new Backend(new InetSocketAddress("10.1.1.1", 9110), 1, -1));
        assertDoesNotThrow(() -> new Backend(new InetSocketAddress("10.1.1.1", 9110), 1, 0));
    }
}
