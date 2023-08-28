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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

class ClusterOnlineNodesWorker implements EventListener<Void> {

    protected final List<Node> onlineNodes = new CopyOnWriteArrayList<>();

    @Override
    public void accept(Event<Void> event) {
        if (event instanceof NodeEvent nodeEvent) {

            if (nodeEvent instanceof NodeOnlineEvent || nodeEvent instanceof NodeAddedEvent) {
                onlineNodes.add(nodeEvent.node());
            } else {
                onlineNodes.remove(nodeEvent.node());
            }
        }
    }

    @Override
    public String toString() {
        return "ClusterOnlineNodesWorker{" +
                "onlineNodes=" + onlineNodes +
                '}';
    }
}
