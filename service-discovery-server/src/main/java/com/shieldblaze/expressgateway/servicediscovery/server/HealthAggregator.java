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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.stereotype.Component;

/**
 * Aggregates health check results for registered services. Provides a summary
 * of the health status across all registered nodes for monitoring and dashboards.
 *
 * <p>Health check data comes from two sources:</p>
 * <ul>
 *   <li>Client heartbeats (TTL-based liveness)</li>
 *   <li>Active health checks (optional, driven by the heartbeat endpoint)</li>
 * </ul>
 */
@Component
public final class HealthAggregator {

    private static final Logger logger = LogManager.getLogger(HealthAggregator.class);

    private final RegistrationStore registrationStore;

    public HealthAggregator(RegistrationStore registrationStore) {
        this.registrationStore = registrationStore;
    }

    /**
     * Compute a health summary of all registered services.
     */
    public HealthSummary summarize() {
        var all = registrationStore.getAll();
        int total = all.size();
        int healthy = 0;
        int unhealthy = 0;
        int expired = 0;

        for (var entry : all) {
            if (entry.isExpired()) {
                expired++;
            } else if (entry.healthy()) {
                healthy++;
            } else {
                unhealthy++;
            }
        }

        return new HealthSummary(total, healthy, unhealthy, expired);
    }

    /**
     * Process a heartbeat for a specific node ID.
     *
     * @return true if the node was found and heartbeat was recorded
     */
    public boolean processHeartbeat(String nodeId) {
        boolean updated = registrationStore.heartbeat(nodeId);
        if (updated) {
            logger.debug("Heartbeat received for node: {}", LogSanitizer.sanitize(nodeId));
        } else {
            logger.warn("Heartbeat for unknown node: {}", LogSanitizer.sanitize(nodeId));
        }
        return updated;
    }

    /**
     * Mark a node as unhealthy (e.g., when an active health check fails).
     */
    public void markUnhealthy(String nodeId) {
        registrationStore.markUnhealthy(nodeId);
        logger.info("Marked node as unhealthy: {}", LogSanitizer.sanitize(nodeId));
    }

    /**
     * Summary record for the health status of all registered services.
     */
    public record HealthSummary(int total, int healthy, int unhealthy, int expired) {
    }
}
