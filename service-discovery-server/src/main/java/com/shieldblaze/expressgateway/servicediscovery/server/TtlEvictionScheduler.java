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

import lombok.RequiredArgsConstructor;
import lombok.extern.slf4j.Slf4j;
import org.apache.curator.x.discovery.ServiceDiscovery;
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
@Slf4j
@Component
@EnableScheduling
@RequiredArgsConstructor
public final class TtlEvictionScheduler {

    private final RegistrationStore registrationStore;
    private final ServiceDiscovery<Node> serviceDiscovery;

    /**
     * Evict expired entries every 10 seconds.
     *
     * <p>For each expired entry, we attempt to unregister from ZooKeeper first.
     * Only entries that were successfully unregistered (or where the ZK entry was
     * already gone) are evicted from the local store to avoid orphaned ZK entries.</p>
     */
    @Scheduled(fixedRate = 10_000)
    public void evictExpired() {
        try {
            var expired = registrationStore.getAll().stream()
                    .filter(RegistrationEntry::isExpired)
                    .toList();

            if (expired.isEmpty()) {
                return;
            }

            int evicted = 0;
            for (var entry : expired) {
                boolean zkCleanedUp = false;
                try {
                    serviceDiscovery.unregisterService(Handler.instance(entry.node()));
                    zkCleanedUp = true;
                    log.info("Unregistered expired node from ZooKeeper: {}", entry.node().id());
                } catch (Exception ex) {
                    // If the ZK node is already gone (e.g., session expired), treat as success
                    String msg = ex.getMessage();
                    if (msg != null && (msg.contains("NoNode") || msg.contains("not found"))) {
                        zkCleanedUp = true;
                        log.debug("Expired node {} already removed from ZooKeeper", entry.node().id());
                    } else {
                        log.warn("Failed to unregister expired node {} from ZooKeeper, will retry: {}",
                                entry.node().id(), msg);
                    }
                }

                if (zkCleanedUp) {
                    registrationStore.deregister(entry.node().id());
                    evicted++;
                }
            }

            if (evicted > 0) {
                log.info("Evicted {} expired registration(s)", evicted);
            }
        } catch (Exception ex) {
            log.error("Error during TTL eviction", ex);
        }
    }
}
