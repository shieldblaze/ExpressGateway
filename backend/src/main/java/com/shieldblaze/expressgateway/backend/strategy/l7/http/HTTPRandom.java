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
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeTask;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.concurrent.task.Task;

import java.io.IOException;
import java.util.concurrent.ThreadLocalRandom;

/**
 * Select {@link Node} Randomly
 */
public final class HTTPRandom extends HTTPBalance {

    // BUG-HTTPRANDOM-THREAD: SplittableRandom is NOT thread-safe. Using a single
    // instance across multiple Netty EventLoop threads causes data races (not just
    // contention — SplittableRandom has no synchronization and can produce correlated
    // sequences or even identical values under concurrent access). ThreadLocalRandom
    // is designed for exactly this use case: one instance per thread, no contention.

    /**
     * Create {@link HTTPRandom} Instance
     *
     * @param sessionPersistence {@link SessionPersistence} Implementation Instance
     */
    public HTTPRandom(SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public String name() {
        return "HTTPRandom";
    }

    @Override
    public HTTPBalanceResponse balance(HTTPBalanceRequest request) throws LoadBalanceException {
        HTTPBalanceResponse httpBalanceResponse = sessionPersistence.node(request);
        if (httpBalanceResponse != null) {
            // If Backend is ONLINE then return the response
            // else remove it from session persistence.
            if (httpBalanceResponse.node().state() == State.ONLINE) {
                return httpBalanceResponse;
            } else {
                sessionPersistence.removeRoute(request, httpBalanceResponse.node());
            }
        }

        Node node;
        var nodes = cluster.onlineNodes();
        if (nodes.isEmpty()) {
            throw new NoNodeAvailableException();
        }
        node = nodes.get(ThreadLocalRandom.current().nextInt(nodes.size()));

        // Add to session persistence and return
        return sessionPersistence.addRoute(request, node);
    }

    @Override
    public void accept(Task task) {
        if (task instanceof NodeTask nodeEvent) {
            if (nodeEvent instanceof NodeOfflineTask || nodeEvent instanceof NodeRemovedTask || nodeEvent instanceof NodeIdleTask) {
                sessionPersistence.remove(nodeEvent.node());
            }
        }
    }

    @Override
    public String toString() {
        return "HTTPRandom{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }

    @Override
    public void close() throws IOException {
        sessionPersistence.clear();
    }
}
