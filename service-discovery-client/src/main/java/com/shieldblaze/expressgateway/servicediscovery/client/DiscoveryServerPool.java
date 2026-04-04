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

import lombok.extern.slf4j.Slf4j;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Manages a pool of discovery server URIs with health-aware selection and failover.
 * The pool uses round-robin selection with automatic skipping of unhealthy servers.
 *
 * <p>Each server tracks consecutive failures. A server is considered unhealthy when its
 * failure count exceeds the configured threshold. Unhealthy servers are periodically
 * retried to detect recovery.</p>
 */
@Slf4j
public final class DiscoveryServerPool {

    private final List<ServerState> servers;
    private final AtomicInteger currentIndex = new AtomicInteger(0);
    private final int unhealthyThreshold;

    /**
     * @param serverUris         list of discovery server base URIs
     * @param unhealthyThreshold consecutive failures before a server is considered unhealthy
     */
    public DiscoveryServerPool(List<String> serverUris, int unhealthyThreshold) {
        if (serverUris == null || serverUris.isEmpty()) {
            throw new IllegalArgumentException("serverUris must not be null or empty");
        }
        if (unhealthyThreshold < 1) {
            throw new IllegalArgumentException("unhealthyThreshold must be >= 1");
        }
        this.servers = serverUris.stream().map(ServerState::new).toList();
        this.unhealthyThreshold = unhealthyThreshold;
    }

    /**
     * Select the next healthy server URI using round-robin. If all servers are
     * unhealthy, the least-recently-failed server is returned as a last resort.
     *
     * @return the selected server URI
     */
    public String selectServer() {
        int size = servers.size();
        int startIdx = Math.floorMod(currentIndex.getAndIncrement(), size);

        // First pass: find a healthy server
        for (int i = 0; i < size; i++) {
            ServerState server = servers.get((startIdx + i) % size);
            if (server.isHealthy(unhealthyThreshold)) {
                return server.uri;
            }
        }

        // All servers unhealthy: return the one with oldest failure (most likely recovered)
        ServerState oldest = servers.getFirst();
        for (int i = 1; i < size; i++) {
            if (servers.get(i).lastFailureTimestamp.get() < oldest.lastFailureTimestamp.get()) {
                oldest = servers.get(i);
            }
        }
        return oldest.uri;
    }

    /**
     * Record a successful call to the given server URI.
     */
    public void recordSuccess(String uri) {
        findServer(uri).ifPresent(s -> s.consecutiveFailures.set(0));
    }

    /**
     * Record a failed call to the given server URI.
     */
    public void recordFailure(String uri) {
        findServer(uri).ifPresent(s -> {
            s.consecutiveFailures.incrementAndGet();
            s.lastFailureTimestamp.set(System.currentTimeMillis());
        });
    }

    /**
     * Return the list of all server URIs.
     */
    public List<String> serverUris() {
        return servers.stream().map(s -> s.uri).toList();
    }

    /**
     * Return the number of currently healthy servers.
     */
    public int healthyCount() {
        return (int) servers.stream().filter(s -> s.isHealthy(unhealthyThreshold)).count();
    }

    /**
     * Return the total number of servers.
     */
    public int size() {
        return servers.size();
    }

    /**
     * Reset all servers to healthy state.
     */
    public void resetAll() {
        servers.forEach(s -> {
            s.consecutiveFailures.set(0);
            s.lastFailureTimestamp.set(0);
        });
    }

    private java.util.Optional<ServerState> findServer(String uri) {
        return servers.stream().filter(s -> s.uri.equals(uri)).findFirst();
    }

    private static final class ServerState {
        final String uri;
        final AtomicInteger consecutiveFailures = new AtomicInteger(0);
        final AtomicLong lastFailureTimestamp = new AtomicLong(0);

        ServerState(String uri) {
            this.uri = uri;
        }

        boolean isHealthy(int threshold) {
            return consecutiveFailures.get() < threshold;
        }
    }
}
