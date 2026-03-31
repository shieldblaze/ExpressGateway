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
package com.shieldblaze.expressgateway.testing;

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

import java.util.HashSet;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PortAllocatorTest {

    @AfterEach
    void cleanup() {
        PortAllocator.releaseAll();
    }

    @Test
    void allocateSinglePort() {
        int port = PortAllocator.allocate();
        assertTrue(port > 0, "Port must be positive");
        assertTrue(port <= 65535, "Port must be <= 65535");
    }

    @Test
    void allocateMultiplePorts() {
        int[] ports = PortAllocator.allocate(5);
        assertEquals(5, ports.length);

        Set<Integer> unique = new HashSet<>();
        for (int port : ports) {
            assertTrue(port > 0);
            unique.add(port);
        }
        assertEquals(5, unique.size(), "All allocated ports must be unique");
    }

    @Test
    void allocateWithInvalidCount() {
        assertThrows(IllegalArgumentException.class, () -> PortAllocator.allocate(0));
        assertThrows(IllegalArgumentException.class, () -> PortAllocator.allocate(-1));
    }

    @Test
    void releaseAndReallocate() {
        int port = PortAllocator.allocate();
        PortAllocator.release(port);
        // After release, the port tracking set no longer contains it
        // (the OS may or may not give us the same port back)
        int port2 = PortAllocator.allocate();
        assertTrue(port2 > 0);
    }

    @Test
    void concurrentAllocation() throws InterruptedException {
        Set<Integer> allPorts = java.util.concurrent.ConcurrentHashMap.newKeySet();
        int threads = 10;
        int portsPerThread = 5;
        java.util.concurrent.CountDownLatch latch = new java.util.concurrent.CountDownLatch(threads);

        for (int i = 0; i < threads; i++) {
            Thread.ofVirtual().start(() -> {
                try {
                    for (int j = 0; j < portsPerThread; j++) {
                        allPorts.add(PortAllocator.allocate());
                    }
                } finally {
                    latch.countDown();
                }
            });
        }

        latch.await();
        assertEquals(threads * portsPerThread, allPorts.size(),
                "All concurrently allocated ports must be unique");
    }
}
