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

import com.shieldblaze.expressgateway.common.utils.LogSanitizer;
import lombok.extern.slf4j.Slf4j;
import org.springframework.stereotype.Component;

import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * In-memory registration store for service discovery entries with TTL support.
 * This store sits alongside the ZooKeeper-backed Curator discovery and provides
 * local metadata (health, TTL) that is not stored in ZooKeeper.
 *
 * <p>Thread-safe for concurrent registration/deregistration from multiple HTTP handlers.</p>
 */
@Slf4j
@Component
public final class RegistrationStore {

    private final Map<String, RegistrationEntry> entries = new ConcurrentHashMap<>();

    /**
     * Register or update a node with the given TTL.
     */
    public RegistrationEntry register(Node node, long ttlSeconds) {
        return entries.compute(node.id(), (key, existing) -> {
            if (existing != null) {
                log.info("Updating existing registration for node: {}", key);
                return existing.withHeartbeat();
            }
            log.info("New registration for node: {}", key);
            return RegistrationEntry.create(node, ttlSeconds);
        });
    }

    /**
     * Deregister a node by ID.
     *
     * @return the removed entry, or null if not found
     */
    public RegistrationEntry deregister(String nodeId) {
        RegistrationEntry removed = entries.remove(nodeId);
        if (removed != null) {
            log.info("Deregistered node: {}", LogSanitizer.sanitize(nodeId));
        }
        return removed;
    }

    /**
     * Get a registration entry by node ID.
     */
    public Optional<RegistrationEntry> get(String nodeId) {
        return Optional.ofNullable(entries.get(nodeId));
    }

    /**
     * Get all registration entries (including expired).
     */
    public Collection<RegistrationEntry> getAll() {
        return entries.values();
    }

    /**
     * Get only healthy, non-expired entries.
     */
    public List<RegistrationEntry> getHealthy() {
        return entries.values().stream()
                .filter(e -> e.healthy() && !e.isExpired())
                .toList();
    }

    /**
     * Send a heartbeat for the given node ID, updating its lastHeartbeat timestamp.
     * Uses computeIfPresent for atomic read-modify-write to avoid race conditions.
     *
     * @return true if the node was found and updated
     */
    public boolean heartbeat(String nodeId) {
        RegistrationEntry updated = entries.computeIfPresent(nodeId,
                (key, existing) -> existing.withHeartbeat());
        return updated != null;
    }

    /**
     * Evict all expired entries and return the count of evicted entries.
     * Re-checks expiry atomically at removal time to avoid TOCTOU races
     * where a heartbeat arrives between the initial scan and the removal.
     */
    public int evictExpired() {
        List<String> candidates = entries.entrySet().stream()
                .filter(e -> e.getValue().isExpired())
                .map(Map.Entry::getKey)
                .toList();

        int evicted = 0;
        for (String key : candidates) {
            // computeIfPresent returns null when the remapping function returns null (entry removed),
            // but also returns null if the key was already absent. Track removal via a flag.
            boolean[] removed = {false};
            entries.computeIfPresent(key, (k, v) -> {
                if (v.isExpired()) {
                    removed[0] = true;
                    return null;
                }
                return v;
            });
            if (removed[0]) {
                log.info("Evicted expired registration: {}", key);
                evicted++;
            }
        }
        return evicted;
    }

    /**
     * Mark a node as unhealthy.
     * Uses computeIfPresent for atomic read-modify-write.
     *
     * @return true if the node was found and updated
     */
    public boolean markUnhealthy(String nodeId) {
        RegistrationEntry updated = entries.computeIfPresent(nodeId,
                (key, existing) -> existing.asUnhealthy());
        return updated != null;
    }

    /**
     * Return the total number of registered entries.
     */
    public int size() {
        return entries.size();
    }

    /**
     * Clear all entries. Used primarily for testing.
     */
    public void clear() {
        entries.clear();
    }
}
