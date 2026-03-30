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

import java.io.IOException;
import java.net.ServerSocket;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Thread-safe port allocator for tests. Guarantees unique, available ports
 * across concurrent test execution by binding to port 0 and tracking
 * allocated ports to prevent double allocation within the same JVM.
 */
public final class PortAllocator {

    private static final Set<Integer> ALLOCATED = ConcurrentHashMap.newKeySet();

    private PortAllocator() {
    }

    /**
     * Allocate a single available TCP port.
     *
     * @return an available port number
     * @throws IllegalStateException if no port can be allocated
     */
    public static int allocate() {
        for (int attempt = 0; attempt < 50; attempt++) {
            try (ServerSocket socket = new ServerSocket(0)) {
                socket.setReuseAddress(true);
                int port = socket.getLocalPort();
                if (ALLOCATED.add(port)) {
                    return port;
                }
            } catch (IOException ignored) {
                // Try again
            }
        }
        throw new IllegalStateException("Unable to allocate a free port after 50 attempts");
    }

    /**
     * Allocate multiple available TCP ports.
     *
     * @param count number of ports to allocate
     * @return array of available port numbers
     * @throws IllegalArgumentException if count is not positive
     */
    public static int[] allocate(int count) {
        if (count <= 0) {
            throw new IllegalArgumentException("count must be positive, got: " + count);
        }
        int[] ports = new int[count];
        for (int i = 0; i < count; i++) {
            ports[i] = allocate();
        }
        return ports;
    }

    /**
     * Release a previously allocated port back to the pool.
     *
     * @param port the port to release
     */
    public static void release(int port) {
        ALLOCATED.remove(port);
    }

    /**
     * Release all allocated ports. Useful in {@code @AfterAll} methods.
     */
    public static void releaseAll() {
        ALLOCATED.clear();
    }
}
