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

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;
import java.time.Instant;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Manages automatic failover between regions when a region becomes unhealthy.
 *
 * <p>The policy supports:</p>
 * <ul>
 *   <li>Configurable failover priority list (ordered region preferences)</li>
 *   <li>Drain-before-failover: waits for existing connections to complete before switching</li>
 *   <li>Automatic failback when the original region recovers</li>
 *   <li>Configurable cooldown between failover events to prevent flapping</li>
 * </ul>
 *
 * <p>This class coordinates with {@link RegionManager} for health information
 * and latency-based failover ordering.</p>
 *
 * <p>Thread safety: all public methods are safe for concurrent use.</p>
 */
public final class RegionFailoverPolicy {

    private static final Logger logger = LogManager.getLogger(RegionFailoverPolicy.class);

    /**
     * Failover event type.
     */
    public enum FailoverType {
        FAILOVER,
        FAILBACK,
        DRAIN_STARTED,
        DRAIN_COMPLETED
    }

    /**
     * A recorded failover event.
     *
     * @param type       the failover event type
     * @param fromRegion the region being failed over from
     * @param toRegion   the region being failed over to
     * @param timestamp  when the event occurred
     * @param reason     human-readable reason for the failover
     */
    public record FailoverEvent(
            FailoverType type,
            String fromRegion,
            String toRegion,
            Instant timestamp,
            String reason
    ) {
        public FailoverEvent {
            Objects.requireNonNull(type, "type");
            Objects.requireNonNull(fromRegion, "fromRegion");
            Objects.requireNonNull(toRegion, "toRegion");
            Objects.requireNonNull(timestamp, "timestamp");
            Objects.requireNonNull(reason, "reason");
        }
    }

    /**
     * Listener for failover events.
     */
    @FunctionalInterface
    public interface FailoverListener {
        void onFailover(FailoverEvent event);
    }

    private final RegionManager regionManager;
    private final List<String> failoverPriority;
    private final Duration drainTimeout;
    private final Duration cooldownPeriod;
    private final boolean autoFailbackEnabled;
    private final ScheduledExecutorService scheduler;

    private final ConcurrentHashMap<String, AtomicReference<String>> activeFailovers = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, Instant> lastFailoverTime = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, Boolean> drainingRegions = new ConcurrentHashMap<>();
    private final java.util.concurrent.CopyOnWriteArrayList<FailoverListener> listeners =
            new java.util.concurrent.CopyOnWriteArrayList<>();

    /**
     * Creates a new failover policy.
     *
     * @param regionManager       the region manager for health and topology information
     * @param failoverPriority    ordered list of preferred failover regions (may be empty for latency-based)
     * @param drainTimeout        maximum time to wait for drain to complete before forcing failover
     * @param cooldownPeriod      minimum time between consecutive failovers for the same region
     * @param autoFailbackEnabled whether to automatically fail back when the original region recovers
     */
    public RegionFailoverPolicy(
            RegionManager regionManager,
            List<String> failoverPriority,
            Duration drainTimeout,
            Duration cooldownPeriod,
            boolean autoFailbackEnabled) {
        this.regionManager = Objects.requireNonNull(regionManager, "regionManager");
        this.failoverPriority = Collections.unmodifiableList(List.copyOf(failoverPriority));
        this.drainTimeout = Objects.requireNonNull(drainTimeout, "drainTimeout");
        this.cooldownPeriod = Objects.requireNonNull(cooldownPeriod, "cooldownPeriod");
        this.autoFailbackEnabled = autoFailbackEnabled;
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "failover-drain-scheduler");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Adds a listener for failover events.
     */
    public void addListener(FailoverListener listener) {
        Objects.requireNonNull(listener, "listener");
        listeners.add(listener);
    }

    /**
     * Evaluates whether a failover should be triggered for the given region.
     * If the region is unhealthy and cooldown has elapsed, triggers failover
     * to the best available target region.
     *
     * <p>Uses CAS to prevent concurrent failover for the same region.</p>
     *
     * <p>Returns a {@link CompletableFuture} that completes after the drain timeout
     * has elapsed and the failover is finalized.</p>
     *
     * @param failedRegion the region that has become unhealthy
     * @return a future containing the target region for failover, or empty if no failover is needed/possible
     */
    public CompletableFuture<Optional<String>> evaluateFailoverAsync(String failedRegion) {
        Objects.requireNonNull(failedRegion, "failedRegion");

        // Use CAS to atomically claim this failover. Only one thread can win.
        AtomicReference<String> ref = activeFailovers.computeIfAbsent(failedRegion, k -> new AtomicReference<>());
        // If already failed over, return immediately
        if (ref.get() != null) {
            logger.debug("Region {} already failed over to {}", failedRegion, ref.get());
            return CompletableFuture.completedFuture(Optional.empty());
        }

        // Check cooldown
        Instant lastFailover = lastFailoverTime.get(failedRegion);
        if (lastFailover != null && Duration.between(lastFailover, Instant.now()).compareTo(cooldownPeriod) < 0) {
            logger.debug("Failover cooldown active for region {}", failedRegion);
            return CompletableFuture.completedFuture(Optional.empty());
        }

        // Find best failover target
        Optional<String> target = selectFailoverTarget(failedRegion);
        if (target.isEmpty()) {
            logger.warn("No available failover target for region {}", failedRegion);
            return CompletableFuture.completedFuture(Optional.empty());
        }

        String targetRegion = target.get();

        // CAS: try to set the target atomically. If someone else won, bail out.
        if (!ref.compareAndSet(null, targetRegion)) {
            logger.debug("Concurrent failover for region {} already in progress (target: {})",
                    failedRegion, ref.get());
            return CompletableFuture.completedFuture(Optional.empty());
        }

        // Record drain start
        drainingRegions.put(failedRegion, true);
        fireEvent(new FailoverEvent(FailoverType.DRAIN_STARTED, failedRegion, targetRegion,
                Instant.now(), "Region " + failedRegion + " unhealthy, draining before failover"));

        // Schedule drain completion after the drain timeout
        CompletableFuture<Optional<String>> future = new CompletableFuture<>();
        scheduler.schedule(() -> {
            try {
                completeDrain(failedRegion, targetRegion);
                future.complete(Optional.of(targetRegion));
            } catch (Exception e) {
                logger.error("Error completing drain for region {}", failedRegion, e);
                future.completeExceptionally(e);
            }
        }, drainTimeout.toMillis(), TimeUnit.MILLISECONDS);

        return future;
    }

    /**
     * Synchronous convenience method for evaluating failover. Blocks until drain completes.
     *
     * @param failedRegion the region that has become unhealthy
     * @return the target region for failover, or empty if no failover is needed/possible
     */
    public Optional<String> evaluateFailover(String failedRegion) {
        try {
            return evaluateFailoverAsync(failedRegion).get();
        } catch (Exception e) {
            logger.error("Error during failover evaluation for region {}", failedRegion, e);
            return Optional.empty();
        }
    }

    /**
     * Completes the drain phase and fires the remaining events.
     */
    private void completeDrain(String failedRegion, String targetRegion) {
        drainingRegions.remove(failedRegion);
        lastFailoverTime.put(failedRegion, Instant.now());

        fireEvent(new FailoverEvent(FailoverType.DRAIN_COMPLETED, failedRegion, targetRegion,
                Instant.now(), "Drain completed for region " + failedRegion));

        fireEvent(new FailoverEvent(FailoverType.FAILOVER, failedRegion, targetRegion,
                Instant.now(), "Failed over from " + failedRegion + " to " + targetRegion));

        logger.info("Executed failover: {} -> {}", failedRegion, targetRegion);
    }

    /**
     * Evaluates whether a failback should be triggered for a previously failed-over region.
     * Only applies if auto-failback is enabled and the original region is healthy again.
     *
     * @param originalRegion the region that was originally failed over from
     * @return true if failback was executed
     */
    public boolean evaluateFailback(String originalRegion) {
        Objects.requireNonNull(originalRegion, "originalRegion");

        if (!autoFailbackEnabled) {
            return false;
        }

        AtomicReference<String> failoverRef = activeFailovers.get(originalRegion);
        if (failoverRef == null || failoverRef.get() == null) {
            return false; // Not currently failed over
        }

        if (!regionManager.isHealthy(originalRegion)) {
            return false; // Original region still unhealthy
        }

        // Check cooldown
        Instant lastFailover = lastFailoverTime.get(originalRegion);
        if (lastFailover != null && Duration.between(lastFailover, Instant.now()).compareTo(cooldownPeriod) < 0) {
            logger.debug("Failback cooldown active for region {}", originalRegion);
            return false;
        }

        String failoverTarget = failoverRef.getAndSet(null);
        if (failoverTarget == null) {
            return false; // Concurrent failback already happened
        }
        lastFailoverTime.put(originalRegion, Instant.now());

        fireEvent(new FailoverEvent(FailoverType.FAILBACK, failoverTarget, originalRegion,
                Instant.now(), "Region " + originalRegion + " recovered, failing back from " + failoverTarget));

        logger.info("Executed failback: {} -> {} (original region recovered)", failoverTarget, originalRegion);
        return true;
    }

    /**
     * Returns the current failover target for a region, if it is actively failed over.
     *
     * @param region the region to check
     * @return the failover target region, or empty if not failed over
     */
    public Optional<String> activeFailoverTarget(String region) {
        AtomicReference<String> ref = activeFailovers.get(region);
        if (ref == null) {
            return Optional.empty();
        }
        return Optional.ofNullable(ref.get());
    }

    /**
     * Returns whether a region is currently in the draining state.
     */
    public boolean isDraining(String region) {
        return drainingRegions.getOrDefault(region, false);
    }

    /**
     * Returns the drain timeout duration.
     */
    public Duration drainTimeout() {
        return drainTimeout;
    }

    /**
     * Returns the configured failover priority list.
     */
    public List<String> failoverPriority() {
        return failoverPriority;
    }

    /**
     * Returns all currently active failover mappings (fromRegion -> toRegion).
     */
    public Map<String, String> allActiveFailovers() {
        Map<String, String> result = new java.util.LinkedHashMap<>();
        for (Map.Entry<String, AtomicReference<String>> entry : activeFailovers.entrySet()) {
            String target = entry.getValue().get();
            if (target != null) {
                result.put(entry.getKey(), target);
            }
        }
        return Collections.unmodifiableMap(result);
    }

    /**
     * Selects the best failover target for the given failed region.
     * Prefers configured priority list first, then falls back to latency-based ordering.
     */
    private Optional<String> selectFailoverTarget(String failedRegion) {
        // First: try configured priority list
        for (String candidate : failoverPriority) {
            if (!candidate.equals(failedRegion) && regionManager.isHealthy(candidate)) {
                return Optional.of(candidate);
            }
        }

        // Fallback: latency-ordered healthy regions from RegionManager
        List<RegionManager.RegionState> failoverCandidates = regionManager.failoverOrder();
        for (RegionManager.RegionState candidate : failoverCandidates) {
            if (!candidate.regionId().equals(failedRegion)) {
                return Optional.of(candidate.regionId());
            }
        }

        return Optional.empty();
    }

    private void fireEvent(FailoverEvent event) {
        for (FailoverListener listener : listeners) {
            try {
                listener.onFailover(event);
            } catch (Exception e) {
                logger.warn("Failover listener threw exception", e);
            }
        }
    }
}
