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
package com.shieldblaze.expressgateway.controlplane.region;

import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RegionManagerTest {

    private RegionManager manager;

    @BeforeEach
    void setUp() {
        manager = new RegionManager("us-east-1");
    }

    @Test
    void localRegionIsRegisteredOnCreation() {
        assertTrue(manager.isHealthy("us-east-1"));
        assertEquals("us-east-1", manager.localRegion());
        assertEquals(1, manager.allRegions().size());
    }

    @Test
    void addAndGetRegion() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 3));

        assertTrue(manager.getRegion("eu-west-1").isPresent());
        assertEquals(RegionManager.RegionHealth.HEALTHY, manager.getRegion("eu-west-1").get().health());
        assertEquals(2, manager.allRegions().size());
    }

    @Test
    void healthyRegionsFilter() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));
        manager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.UNHEALTHY, Instant.now(), 0));

        List<RegionManager.RegionState> healthy = manager.healthyRegions();
        assertEquals(2, healthy.size()); // us-east-1 + eu-west-1
    }

    @Test
    void recordAndGetLatency() {
        RegionManager.RegionLatency latency = new RegionManager.RegionLatency(
                "us-east-1", "eu-west-1", 120, Instant.now());
        manager.recordLatency(latency);

        assertTrue(manager.getLatency("us-east-1", "eu-west-1").isPresent());
        assertEquals(120, manager.getLatency("us-east-1", "eu-west-1").get().latencyMs());
        assertTrue(manager.getLatency("eu-west-1", "us-east-1").isEmpty()); // directional
    }

    @Test
    void failoverOrderSortedByLatency() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));
        manager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        manager.recordLatency(new RegionManager.RegionLatency(
                "us-east-1", "eu-west-1", 100, Instant.now()));
        manager.recordLatency(new RegionManager.RegionLatency(
                "us-east-1", "ap-south-1", 200, Instant.now()));

        List<RegionManager.RegionState> failoverOrder = manager.failoverOrder();
        assertEquals(2, failoverOrder.size());
        assertEquals("eu-west-1", failoverOrder.get(0).regionId());
        assertEquals("ap-south-1", failoverOrder.get(1).regionId());
    }

    @Test
    void failoverOrderExcludesUnhealthyRegions() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.UNHEALTHY, Instant.now(), 0));
        manager.updateRegion(new RegionManager.RegionState(
                "ap-south-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        List<RegionManager.RegionState> failoverOrder = manager.failoverOrder();
        assertEquals(1, failoverOrder.size());
        assertEquals("ap-south-1", failoverOrder.get(0).regionId());
    }

    @Test
    void removeRegionCleansUpLatencies() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));
        manager.recordLatency(new RegionManager.RegionLatency(
                "us-east-1", "eu-west-1", 100, Instant.now()));

        assertTrue(manager.removeRegion("eu-west-1"));
        assertFalse(manager.getRegion("eu-west-1").isPresent());
        assertTrue(manager.getLatency("us-east-1", "eu-west-1").isEmpty());
    }

    @Test
    void removeRegionIsAtomic() {
        // After removal, both region and latencies should be gone
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));
        manager.recordLatency(new RegionManager.RegionLatency(
                "us-east-1", "eu-west-1", 100, Instant.now()));
        manager.recordLatency(new RegionManager.RegionLatency(
                "eu-west-1", "us-east-1", 100, Instant.now()));

        assertTrue(manager.removeRegion("eu-west-1"));

        // Both the region and all latency entries should be removed atomically
        assertFalse(manager.getRegion("eu-west-1").isPresent());
        assertTrue(manager.getLatency("us-east-1", "eu-west-1").isEmpty());
        assertTrue(manager.getLatency("eu-west-1", "us-east-1").isEmpty());
    }

    @Test
    void localRegionCannotBeRemoved() {
        assertThrows(IllegalArgumentException.class, () -> manager.removeRegion("us-east-1"));
        // Verify the local region is still present
        assertTrue(manager.isHealthy("us-east-1"));
    }

    @Test
    void updateRegionHealthTransition() {
        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.HEALTHY, Instant.now(), 2));

        manager.updateRegion(new RegionManager.RegionState(
                "eu-west-1", RegionManager.RegionHealth.DEGRADED, Instant.now(), 2));

        assertEquals(RegionManager.RegionHealth.DEGRADED, manager.getRegion("eu-west-1").get().health());
    }
}
