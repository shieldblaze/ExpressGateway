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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

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
import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import com.shieldblaze.expressgateway.concurrent.task.Task;

import java.io.IOException;
import java.util.List;

/**
 * <p> Select {@link Node} with the lowest EWMA response time. </p>
 *
 * <p> Response time is tracked per-node using Exponentially Weighted Moving Average
 * (EWMA) via {@link ResponseTimeTracker}. The node with the lowest EWMA value is
 * selected for each request. </p>
 *
 * <p> Cold start behavior: Nodes that have not yet accumulated
 * {@value ResponseTimeTracker#COLD_START_THRESHOLD} samples are considered "cold".
 * When all online nodes are cold, a round-robin fallback is used to distribute
 * requests evenly and gather initial samples. When some nodes are warm and others
 * are cold, cold nodes are preferred (to gather samples) unless all cold nodes
 * are offline. </p>
 *
 * <p> Response time recording: Call {@link #recordResponseTime(Node, long)} after
 * each backend response to update the EWMA. This is typically done in the response
 * handler of the proxy pipeline. </p>
 *
 * <p> Thread safety: The {@link ResponseTimeTracker} uses lock-free CAS internally.
 * The node selection logic in {@link #balance(HTTPBalanceRequest)} uses only a
 * for-loop over the online nodes list (which is a snapshot from
 * {@link com.shieldblaze.expressgateway.backend.cluster.Cluster#onlineNodes()}).
 * No synchronization is needed on the hot path. </p>
 */
public final class HTTPLeastResponseTime extends HTTPBalance {

    private final ResponseTimeTracker tracker;
    private final RoundRobinIndexGenerator roundRobinIndexGenerator = new RoundRobinIndexGenerator(0);

    /**
     * Create {@link HTTPLeastResponseTime} Instance with default alpha (0.5).
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public HTTPLeastResponseTime(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
        this(sessionPersistence, new ResponseTimeTracker());
    }

    /**
     * Create {@link HTTPLeastResponseTime} Instance with a custom {@link ResponseTimeTracker}.
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     * @param tracker            the response time tracker to use
     */
    public HTTPLeastResponseTime(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence,
                                 ResponseTimeTracker tracker) {
        super(sessionPersistence);
        this.tracker = tracker;
    }

    @Override
    public String name() {
        return "HTTPLeastResponseTime";
    }

    @Override
    public HTTPBalanceResponse balance(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.node(request);
        if (httpBalanceResponse != null) {
            if (httpBalanceResponse.node().state() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.node());
            }
        }

        List<Node> onlineNodes = cluster.onlineNodes();
        if (onlineNodes.isEmpty()) {
            throw NoNodeAvailableException.INSTANCE;
        }

        Node node = selectNode(onlineNodes);

        return sessionPersistence.addRoute(request, node);
    }

    /**
     * Record a response time sample for the given node. This should be called
     * after each backend response is received.
     *
     * @param node               the backend node that served the request
     * @param responseTimeMillis the response time in milliseconds
     */
    public void recordResponseTime(Node node, long responseTimeMillis) {
        tracker.record(node, responseTimeMillis);
    }

    /**
     * Get the {@link ResponseTimeTracker} used by this load balancer.
     * Useful for diagnostics and monitoring.
     *
     * @return the response time tracker
     */
    public ResponseTimeTracker tracker() {
        return tracker;
    }

    @Override
    public void accept(Task task) {
        if (task instanceof NodeTask nodeEvent) {
            if (nodeEvent instanceof NodeOfflineTask || nodeEvent instanceof NodeRemovedTask || nodeEvent instanceof NodeIdleTask) {
                sessionPersistence.remove(nodeEvent.node());
                tracker.remove(nodeEvent.node());
                roundRobinIndexGenerator.decMaxIndex();
            } else if (nodeEvent instanceof NodeOnlineTask || nodeEvent instanceof NodeAddedTask) {
                roundRobinIndexGenerator.incMaxIndex();
            }
        }
    }

    @Override
    public String toString() {
        return "HTTPLeastResponseTime{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }

    @Override
    public void close() throws IOException {
        sessionPersistence.clear();
        tracker.clear();
    }

    /**
     * Select the best node from the online nodes list.
     *
     * <p> Strategy: </p>
     * <ol>
     *     <li>If ALL nodes are cold (below sample threshold), use round-robin to
     *         distribute requests evenly and gather initial samples.</li>
     *     <li>If SOME nodes are cold, prefer cold nodes to gather samples faster.
     *         This uses round-robin among only the cold nodes.</li>
     *     <li>If ALL nodes are warm, select the node with the lowest EWMA.</li>
     * </ol>
     *
     * @param onlineNodes the list of online nodes (guaranteed non-empty)
     * @return the selected node
     */
    private Node selectNode(List<Node> onlineNodes) {
        int warmCount = 0;
        int coldCount = 0;

        for (int i = 0, size = onlineNodes.size(); i < size; i++) {
            if (tracker.isWarm(onlineNodes.get(i))) {
                warmCount++;
            } else {
                coldCount++;
            }
        }

        // Case 1: All nodes are cold -- round-robin.
        if (warmCount == 0) {
            int index = roundRobinIndexGenerator.next();
            return onlineNodes.get(Math.floorMod(index, onlineNodes.size()));
        }

        // Case 2: Some nodes are cold -- prefer cold nodes for sample gathering.
        // Use round-robin among cold nodes only.
        if (coldCount > 0) {
            int index = roundRobinIndexGenerator.next();
            int coldIdx = Math.floorMod(index, coldCount);
            int seen = 0;
            for (int i = 0, size = onlineNodes.size(); i < size; i++) {
                Node candidate = onlineNodes.get(i);
                if (!tracker.isWarm(candidate)) {
                    if (seen == coldIdx) {
                        return candidate;
                    }
                    seen++;
                }
            }
            // Fallthrough: should not happen, but if it does, fall through to EWMA selection.
        }

        // Case 3: All nodes are warm -- select lowest EWMA.
        Node best = onlineNodes.get(0);
        double bestEwma = tracker.getEwma(best);
        for (int i = 1, size = onlineNodes.size(); i < size; i++) {
            Node candidate = onlineNodes.get(i);
            double candidateEwma = tracker.getEwma(candidate);
            if (candidateEwma < bestEwma) {
                bestEwma = candidateEwma;
                best = candidate;
            }
        }
        return best;
    }
}
