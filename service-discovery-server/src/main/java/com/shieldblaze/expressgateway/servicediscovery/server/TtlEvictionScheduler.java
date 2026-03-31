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

import org.apache.curator.x.discovery.ServiceDiscovery;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.springframework.scheduling.annotation.EnableScheduling;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Component;

/**
 * Scheduled task that periodically evicts expired registrations from the
 * {@link RegistrationStore} and unregisters them from the ZooKeeper-backed
 * Curator service discovery. Runs every 10 seconds by default.
 *
 * <p>This ensures that nodes whose TTL has expired (no heartbeat received
 * within the TTL window) are automatically cleaned up from the service registry.</p>
 */
@Component
@EnableScheduling
public final class TtlEvictionScheduler {

    private static final Logger logger = LogManager.getLogger(TtlEvictionScheduler.class);

    private final RegistrationStore registrationStore;
    private final ServiceDiscovery<Node> serviceDiscovery;

    public TtlEvictionScheduler(RegistrationStore registrationStore,
                                ServiceDiscovery<Node> serviceDiscovery) {
        this.registrationStore = registrationStore;
        this.serviceDiscovery = serviceDiscovery;
    }

    /**
     * Evict expired entries every 10 seconds.
     */
    @Scheduled(fixedRate = 10_000)
    public void evictExpired() {
        try {
            // Get expired entries before evicting (so we can unregister from ZK)
            var expired = registrationStore.getAll().stream()
                    .filter(RegistrationEntry::isExpired)
                    .toList();

            if (expired.isEmpty()) {
                return;
            }

            for (var entry : expired) {
                try {
                    serviceDiscovery.unregisterService(Handler.instance(entry.node()));
                    logger.info("Unregistered expired node from ZooKeeper: {}", entry.node().id());
                } catch (Exception ex) {
                    logger.warn("Failed to unregister expired node {} from ZooKeeper: {}",
                            entry.node().id(), ex.getMessage());
                }
            }

            int evicted = registrationStore.evictExpired();
            if (evicted > 0) {
                logger.info("Evicted {} expired registration(s)", evicted);
            }
        } catch (Exception ex) {
            logger.error("Error during TTL eviction", ex);
        }
    }
}
