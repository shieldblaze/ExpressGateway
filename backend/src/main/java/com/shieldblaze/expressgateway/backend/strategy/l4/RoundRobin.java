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
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleTask;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineTask;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.exceptions.NoNodeAvailableException;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;
import com.shieldblaze.expressgateway.concurrent.task.Task;

import java.io.IOException;
import java.net.InetSocketAddress;

public final class RoundRobin extends L4Balance {

    private final RoundRobinIndexGenerator roundRobinIndexGenerator = new RoundRobinIndexGenerator(0);

    public RoundRobin(SessionPersistence<Node, Node, InetSocketAddress, Node> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public String name() {
        return "RoundRobin";
    }

    @Override
    public L4Response balance(L4Request l4Request) throws LoadBalanceException {
        Node node = sessionPersistence.node(l4Request);
        if (node != null) {
            if (node.state() == State.ONLINE) {
                return new L4Response(node);
            }
            sessionPersistence.removeRoute(l4Request.socketAddress(), node);
        }

        var onlineNodes = cluster.onlineNodes();
        if (onlineNodes.isEmpty()) {
            throw NoNodeAvailableException.INSTANCE;
        }
        int index = roundRobinIndexGenerator.next();
        node = onlineNodes.get(Math.floorMod(index, onlineNodes.size()));

        sessionPersistence.addRoute(l4Request.socketAddress(), node);
        return new L4Response(node);
    }

    @Override
    public void accept(Task task) {
        if (task instanceof NodeTask nodeEvent) {
            if (nodeEvent instanceof NodeOfflineTask || nodeEvent instanceof NodeRemovedTask || nodeEvent instanceof NodeIdleTask) {
                sessionPersistence.remove(nodeEvent.node());
                roundRobinIndexGenerator.decMaxIndex();
            } else if (nodeEvent instanceof NodeOnlineTask || nodeEvent instanceof NodeAddedTask) {
                roundRobinIndexGenerator.incMaxIndex();
            }
        }
    }

    @Override
    public String toString() {
        return "RoundRobin{" +
                "sessionPersistence=" + sessionPersistence +
                ", cluster=" + cluster +
                '}';
    }

    @Override
    public void close() throws IOException {
        sessionPersistence.clear();
    }
}
