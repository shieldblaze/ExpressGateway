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
package com.shieldblaze.expressgateway.backend.healthcheck;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.healthcheck.OutlierDetectorConfiguration;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;

/**
 * Passive health checking via outlier detection.
 *
 * <p>Monitors backend response success/failure rates passively from actual traffic.
 * When a backend exceeds the consecutive failure threshold, it is ejected
 * (temporarily marked offline). After the ejection time passes, it is set
 * to IDLE for active health checks to verify before returning to ONLINE.</p>
 *
 * <p>Thread-safe: uses {@link LongAdder} for per-node counters and
 * {@link ConcurrentHashMap} for node tracking. The evaluation loop runs
 * on a single scheduled thread.</p>
 *
 * <p>Similar to Envoy's outlier detection feature.</p>
 */
@Log4j2
public final class OutlierDetector implements Closeable {

    private final OutlierDetectorConfiguration config;
    private final EventStream eventStream;
    private final ScheduledExecutorService scheduler;
    private ScheduledFuture<?> evaluationFuture;

    /**
     * Per-node tracking state. Contains counters that are reset every evaluation interval.
     */
    private final Map<Node, NodeCounters> nodeCounters = new ConcurrentHashMap<>();

    /**
     * Nodes currently ejected, mapped to their ejection timestamp (nanos)
     */
    private final Map<Node, Long> ejectedNodes = new ConcurrentHashMap<>();

    /**
     * Total nodes tracked (for max ejection percentage calculation)
     */
    private final List<Node> allNodes;

    public OutlierDetector(OutlierDetectorConfiguration config, EventStream eventStream, List<Node> allNodes) {
        this.config = config;
        this.eventStream = eventStream;
        this.allNodes = allNodes;
        this.scheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "outlier-detector");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Start the periodic evaluation loop
     */
    public void start() {
        if (!config.enabled()) {
            return;
        }
        evaluationFuture = scheduler.scheduleWithFixedDelay(
                this::evaluate,
                config.intervalMs(),
                config.intervalMs(),
                TimeUnit.MILLISECONDS
        );
        log.info("Outlier detector started with interval={}ms, ejectionTime={}ms, failureThreshold={}",
                config.intervalMs(), config.ejectionTimeMs(), config.consecutiveFailures());
    }

    /**
     * Register a node for tracking
     */
    public void addNode(Node node) {
        nodeCounters.put(node, new NodeCounters());
    }

    /**
     * Unregister a node from tracking
     */
    public void removeNode(Node node) {
        nodeCounters.remove(node);
        ejectedNodes.remove(node);
    }

    /**
     * Record a successful response from a backend node.
     * Called from the data path -- must be allocation-free and fast.
     */
    public void recordSuccess(Node node) {
        NodeCounters counters = nodeCounters.get(node);
        if (counters != null) {
            // HIGH-05: Use AtomicLong.set(0) instead of LongAdder.reset() for
            // consecutive failure tracking. LongAdder.reset() is not safe under
            // concurrent updates -- it can lose increments that race with the reset.
            counters.consecutiveFailures.set(0);
            counters.successes.increment();
        }
    }

    /**
     * Record a failed response from a backend node.
     * Called from the data path -- must be allocation-free and fast.
     */
    public void recordFailure(Node node) {
        NodeCounters counters = nodeCounters.get(node);
        if (counters != null) {
            counters.consecutiveFailures.incrementAndGet();
            counters.failures.increment();
        }
    }

    /**
     * Periodic evaluation: check if any nodes should be ejected or restored.
     * Runs on the scheduler thread -- not on the EventLoop hot path.
     */
    private void evaluate() {
        try {
            // Phase 1: Check for nodes to restore from ejection
            long nowNanos = System.nanoTime();
            ejectedNodes.entrySet().removeIf(entry -> {
                long elapsedMs = (nowNanos - entry.getValue()) / 1_000_000;
                if (elapsedMs >= config.ejectionTimeMs()) {
                    Node node = entry.getKey();
                    // Transition to IDLE so active health check can verify before going ONLINE
                    if (node.state() == State.OFFLINE) {
                        node.state(State.IDLE);
                        eventStream.publish(new NodeIdleTask(node));
                        log.info("Outlier detector: restoring ejected node {} to IDLE after {}ms", node.socketAddress(), elapsedMs);
                    }
                    return true;
                }
                return false;
            });

            // Phase 2: Check for nodes to eject
            int totalNodes = allNodes.size();
            int maxEjectable = Math.max(1, (totalNodes * config.maxEjectionPercent()) / 100);
            int currentlyEjected = ejectedNodes.size();

            for (Map.Entry<Node, NodeCounters> entry : nodeCounters.entrySet()) {
                Node node = entry.getKey();
                NodeCounters counters = entry.getValue();

                // Skip nodes already ejected or manually offline
                if (ejectedNodes.containsKey(node) || node.state() == State.MANUAL_OFFLINE) {
                    continue;
                }

                long failures = counters.consecutiveFailures.get();
                if (failures >= config.consecutiveFailures()) {
                    // Check max ejection percentage
                    if (currentlyEjected >= maxEjectable) {
                        log.warn("Outlier detector: node {} exceeded failure threshold ({}) but max ejection limit reached ({}/{})",
                                node.socketAddress(), failures, currentlyEjected, maxEjectable);
                        counters.reset();
                        continue;
                    }

                    // Eject the node
                    node.state(State.OFFLINE);
                    ejectedNodes.put(node, System.nanoTime());
                    eventStream.publish(new NodeOfflineTask(node));
                    currentlyEjected++;

                    log.warn("Outlier detector: ejecting node {} after {} consecutive failures, ejection time={}ms",
                            node.socketAddress(), failures, config.ejectionTimeMs());
                }

                // Reset aggregate counters for next interval.
                // consecutiveFailures is NOT reset here (handled per-node on success).
                counters.reset();
            }
        } catch (Exception ex) {
            log.error("Outlier detector evaluation failed", ex);
        }
    }

    @Override
    public void close() {
        if (evaluationFuture != null) {
            evaluationFuture.cancel(false);
        }
        scheduler.shutdown();
        nodeCounters.clear();
        ejectedNodes.clear();
    }

    /**
     * Per-node counters. Uses LongAdder for aggregate success/failure counts (contention-free
     * from multiple EventLoops), and AtomicLong for consecutive failures because LongAdder.reset()
     * is not safe under concurrent updates (HIGH-05: it can lose increments that race with reset).
     */
    private static final class NodeCounters {
        final LongAdder successes = new LongAdder();
        final LongAdder failures = new LongAdder();
        final AtomicLong consecutiveFailures = new AtomicLong(0);

        void reset() {
            successes.reset();
            failures.reset();
            // Note: consecutiveFailures is NOT reset here -- it tracks
            // consecutive failures across intervals. It is reset on success.
        }
    }
}
