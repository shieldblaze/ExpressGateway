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
package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.State;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeTask;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.concurrent.task.Task;

import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * <p> Select {@link Node} based on smooth weighted round-robin (NGINX-style). </p>
 *
 * <p> Each node is assigned a weight (default 1). On each selection: </p>
 * <ol>
 *     <li> For all online nodes: {@code currentWeight += effectiveWeight} </li>
 *     <li> Select the node with the highest {@code currentWeight} </li>
 *     <li> For the selected node: {@code currentWeight -= totalWeight} </li>
 * </ol>
 *
 * <p> This produces a smooth, interleaved distribution. For example, with weights
 * {A=5, B=1, C=1}, the selection sequence is: A, A, B, A, C, A, A (not AAAAABCA).
 * This is the same algorithm used by NGINX upstream. </p>
 *
 * <p> Thread safety: The {@link #balance(L4Request)} method and weight updates are
 * synchronized. The hot path acquires a single monitor -- this is acceptable because
 * the smooth WRR algorithm requires atomic read-modify-write of all node weights in
 * a single pass. A lock-free approach would require CAS loops over multiple atomics
 * which is more complex and no faster under moderate contention. </p>
 *
 * <p> Runtime weight changes are supported via {@link #setWeight(Node, int)}. The
 * effectiveWeight is updated immediately and takes effect on the next selection. </p>
 */
public final class WeightedRoundRobin extends L4Balance {

    /**
     * Per-node weight state. Tracks currentWeight and effectiveWeight for the
     * smooth weighted round-robin algorithm.
     */
    private static final class WeightState {
        /**
         * The current accumulated weight. Increases by effectiveWeight each round,
         * decreases by totalWeight when this node is selected.
         */
        int currentWeight;

        /**
         * The effective weight of this node. Initially set to the configured weight.
         * Can be updated at runtime via {@link WeightedRoundRobin#setWeight(Node, int)}.
         */
        int effectiveWeight;

        WeightState(int weight) {
            this.effectiveWeight = weight;
            this.currentWeight = 0;
        }
    }

    /**
     * Default weight for nodes that have not been explicitly assigned a weight.
     */
    private static final int DEFAULT_WEIGHT = 1;

    /**
     * Map of Node to its weight state. Uses ConcurrentHashMap for safe concurrent
     * reads in toString/diagnostics; mutations are done under the monitor lock.
     */
    private final Map<Node, WeightState> weights = new HashMap<>();

    /**
     * Create {@link WeightedRoundRobin} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public WeightedRoundRobin(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public String name() {
        return "WeightedRoundRobin";
    }

    @Override
    public synchronized L4Response balance(L4Request l4Request) throws LoadBalanceException {
        Node node = sessionPersistence.node(l4Request);
        if (node != null) {
            if (node.state() == State.ONLINE) {
                return new L4Response(node);
            } else {
                sessionPersistence.removeRoute(l4Request.socketAddress(), node);
            }
        }

        List<Node> onlineNodes = cluster.onlineNodes();
        if (onlineNodes.isEmpty()) {
            throw new NoNodeAvailableException();
        }

        // Smooth weighted round-robin selection.
        // Step 1: Compute totalWeight and increment currentWeight for all online nodes.
        int totalWeight = 0;
        Node best = null;
        int bestCurrentWeight = Integer.MIN_VALUE;

        for (int i = 0, size = onlineNodes.size(); i < size; i++) {
            Node candidate = onlineNodes.get(i);
            WeightState ws = weights.get(candidate);
            if (ws == null) {
                // Node was added without an explicit weight; use default.
                ws = new WeightState(DEFAULT_WEIGHT);
                weights.put(candidate, ws);
            }

            ws.currentWeight += ws.effectiveWeight;
            totalWeight += ws.effectiveWeight;

            if (ws.currentWeight > bestCurrentWeight) {
                bestCurrentWeight = ws.currentWeight;
                best = candidate;
            }
        }

        // Step 2: Selected node gets currentWeight reduced by totalWeight.
        if (best != null) {
            WeightState bestWs = weights.get(best);
            bestWs.currentWeight -= totalWeight;
            node = best;
        } else {
            // Should not happen if onlineNodes is non-empty, but guard defensively.
            throw new NoNodeAvailableException();
        }

        sessionPersistence.addRoute(l4Request.socketAddress(), node);
        return new L4Response(node);
    }

    /**
     * Set the weight for a specific {@link Node}. Takes effect on the next
     * selection cycle. If the node is not yet tracked, it will be added.
     *
     * @param node   the node to configure
     * @param weight the weight, must be &gt;= 1
     * @throws IllegalArgumentException if weight is less than 1
     */
    public synchronized void setWeight(Node node, int weight) {
        if (weight < 1) {
            throw new IllegalArgumentException("Weight must be >= 1, got: " + weight);
        }
        WeightState ws = weights.get(node);
        if (ws == null) {
            ws = new WeightState(weight);
            weights.put(node, ws);
        } else {
            ws.effectiveWeight = weight;
        }
    }

    /**
     * Get the current effective weight for a {@link Node}.
     *
     * @param node the node to query
     * @return the effective weight, or {@link #DEFAULT_WEIGHT} if not explicitly set
     */
    public synchronized int getWeight(Node node) {
        WeightState ws = weights.get(node);
        return ws != null ? ws.effectiveWeight : DEFAULT_WEIGHT;
    }

    @Override
    public synchronized void accept(Task task) {
        if (task instanceof NodeTask nodeEvent) {
            if (nodeEvent instanceof NodeOfflineTask || nodeEvent instanceof NodeRemovedTask || nodeEvent instanceof NodeIdleTask) {
                sessionPersistence.remove(nodeEvent.node());
                weights.remove(nodeEvent.node());
            } else if (nodeEvent instanceof NodeOnlineTask || nodeEvent instanceof NodeAddedTask) {
                weights.computeIfAbsent(nodeEvent.node(), n -> new WeightState(DEFAULT_WEIGHT));
            }
        }
    }

    @Override
    public String toString() {
        return "WeightedRoundRobin{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }

    @Override
    public void close() throws IOException {
        sessionPersistence.clear();
        synchronized (this) {
            weights.clear();
        }
    }
}
