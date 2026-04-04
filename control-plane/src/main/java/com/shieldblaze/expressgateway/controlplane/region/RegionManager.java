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

import lombok.extern.log4j.Log4j2;

import java.time.Instant;
import java.util.Collection;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Tracks region topology, health, and cross-region latency for multi-region deployments.
 *
 * <p>Each region is identified by a unique string (e.g. "us-east-1", "eu-west-1").
 * The manager maintains:</p>
 * <ul>
 *   <li>Region health status and last-check timestamps</li>
 *   <li>Cross-region latency measurements for failover ordering</li>
 *   <li>Failover priority based on measured latency and configured preferences</li>
 * </ul>
 *
 * <p>Thread safety: all operations are backed by {@link ConcurrentHashMap}.</p>
 */
@Log4j2
public final class RegionManager {

    /**
     * Health state of a region.
     */
    public enum RegionHealth {
        HEALTHY,
        DEGRADED,
        UNHEALTHY
    }

    /**
     * Snapshot of a region's current state.
     *
     * @param regionId    unique region identifier
     * @param health      current health state
     * @param lastChecked when the health was last verified
     * @param nodeCount   number of active control plane instances in this region
     */
    public record RegionState(
            String regionId,
            RegionHealth health,
            Instant lastChecked,
            int nodeCount
    ) {
        public RegionState {
            Objects.requireNonNull(regionId, "regionId");
            Objects.requireNonNull(health, "health");
            Objects.requireNonNull(lastChecked, "lastChecked");
            if (nodeCount < 0) {
                throw new IllegalArgumentException("nodeCount must be >= 0");
            }
        }
    }

    /**
     * Measured latency between two regions.
     *
     * @param fromRegion the source region
     * @param toRegion   the destination region
     * @param latencyMs  the measured round-trip latency in milliseconds
     * @param measuredAt when the measurement was taken
     */
    public record RegionLatency(
            String fromRegion,
            String toRegion,
            long latencyMs,
            Instant measuredAt
    ) {
        public RegionLatency {
            Objects.requireNonNull(fromRegion, "fromRegion");
            Objects.requireNonNull(toRegion, "toRegion");
            Objects.requireNonNull(measuredAt, "measuredAt");
            if (latencyMs < 0) {
                throw new IllegalArgumentException("latencyMs must be >= 0");
            }
        }
    }

    private final String localRegion;
    private final ConcurrentHashMap<String, RegionState> regions = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, RegionLatency> latencies = new ConcurrentHashMap<>();

    /**
     * Creates a new region manager for the given local region.
     *
     * @param localRegion the region ID of this control plane instance
     */
    public RegionManager(String localRegion) {
        this.localRegion = Objects.requireNonNull(localRegion, "localRegion");
        if (localRegion.isBlank()) {
            throw new IllegalArgumentException("localRegion must not be blank");
        }
        // Register local region as healthy
        regions.put(localRegion, new RegionState(localRegion, RegionHealth.HEALTHY, Instant.now(), 1));
    }

    /**
     * Returns the local region ID.
     */
    public String localRegion() {
        return localRegion;
    }

    /**
     * Updates the state of a region. Creates the entry if it does not exist.
     *
     * @param state the new region state
     */
    public void updateRegion(RegionState state) {
        Objects.requireNonNull(state, "state");
        RegionState previous = regions.put(state.regionId(), state);
        if (previous == null) {
            log.info("Discovered new region: {}", state.regionId());
        } else if (previous.health() != state.health()) {
            log.info("Region {} health changed: {} -> {}", state.regionId(), previous.health(), state.health());
        }
    }

    /**
     * Records a latency measurement between two regions.
     *
     * @param latency the latency measurement
     */
    public void recordLatency(RegionLatency latency) {
        Objects.requireNonNull(latency, "latency");
        String key = latencyKey(latency.fromRegion(), latency.toRegion());
        latencies.put(key, latency);
    }

    /**
     * Returns the current state of a region, if known.
     *
     * @param regionId the region to look up
     * @return the region state, or empty if the region is unknown
     */
    public Optional<RegionState> getRegion(String regionId) {
        Objects.requireNonNull(regionId, "regionId");
        return Optional.ofNullable(regions.get(regionId));
    }

    /**
     * Returns an unmodifiable view of all known regions.
     */
    public Collection<RegionState> allRegions() {
        return Collections.unmodifiableCollection(regions.values());
    }

    /**
     * Returns all healthy regions.
     */
    public List<RegionState> healthyRegions() {
        return regions.values().stream()
                .filter(r -> r.health() == RegionHealth.HEALTHY)
                .toList();
    }

    /**
     * Returns the measured latency between two regions, if available.
     *
     * @param fromRegion source region
     * @param toRegion   destination region
     * @return the latency measurement, or empty if no measurement exists
     */
    public Optional<RegionLatency> getLatency(String fromRegion, String toRegion) {
        return Optional.ofNullable(latencies.get(latencyKey(fromRegion, toRegion)));
    }

    /**
     * Returns the ordered list of failover target regions from the local region,
     * sorted by measured latency (lowest first). Excludes the local region and
     * unhealthy regions.
     *
     * @return ordered list of failover candidates
     */
    public List<RegionState> failoverOrder() {
        return regions.values().stream()
                .filter(r -> !r.regionId().equals(localRegion))
                .filter(r -> r.health() != RegionHealth.UNHEALTHY)
                .sorted(Comparator.comparingLong(r -> {
                    Optional<RegionLatency> lat = getLatency(localRegion, r.regionId());
                    return lat.map(RegionLatency::latencyMs).orElse(Long.MAX_VALUE);
                }))
                .toList();
    }

    /**
     * Checks if a region is healthy.
     *
     * @param regionId the region to check
     * @return true if the region is known and healthy
     */
    public boolean isHealthy(String regionId) {
        RegionState state = regions.get(regionId);
        return state != null && state.health() == RegionHealth.HEALTHY;
    }

    /**
     * Removes a region from tracking. The local region cannot be removed.
     *
     * <p>The removal of the region entry and associated latency entries is performed
     * atomically using the region's compute lock to prevent observing inconsistent state.</p>
     *
     * @param regionId the region to remove
     * @return true if the region was removed
     * @throws IllegalArgumentException if attempting to remove the local region
     */
    public boolean removeRegion(String regionId) {
        Objects.requireNonNull(regionId, "regionId");
        if (localRegion.equals(regionId)) {
            throw new IllegalArgumentException("Cannot remove local region: " + regionId);
        }
        // Use compute to make the region removal and latency cleanup atomic
        // with respect to other operations on the same region key.
        boolean[] removed = {false};
        regions.compute(regionId, (key, existing) -> {
            if (existing != null) {
                // Remove latency entries involving this region while holding the compute lock
                latencies.keySet().removeIf(k -> k.startsWith(regionId + "->") || k.endsWith("->" + regionId));
                log.info("Removed region: {}", regionId);
                removed[0] = true;
            }
            return null; // return null to remove the entry
        });
        return removed[0];
    }

    private static String latencyKey(String from, String to) {
        return from + "->" + to;
    }
}
