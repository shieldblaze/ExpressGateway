/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.services;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.concurrent.event.Event;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventListener;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Controls and manages {@linkplain Node}
 */
public class BackendControllerService implements EventListener {
    private final Map<Node, NodeServices> nodeServicesMap = new ConcurrentHashMap<>();

    @Override
    public void accept(Event event) {
        if (event instanceof NodeEvent) {
            NodeEvent nodeEvent = (NodeEvent) event;
            if (nodeEvent instanceof NodeAddedEvent || nodeEvent instanceof NodeOnlineEvent) {

                // If NodeAddedEvent is successfully finished and successful
                // then we'll start ConnectionCleaner and Health Check (if applicable) services.
                if (nodeEvent.finished() && nodeEvent.success()) {
                    ScheduledFuture<?> healthCheck = null;
                    ScheduledFuture<?> connectionCleaner;

                    if (nodeEvent.node().healthCheck() != null) {
                        healthCheck = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(new HealthCheckService(nodeEvent.node()), 0, 5,
                                TimeUnit.MILLISECONDS);
                    }

                    connectionCleaner = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(new ConnectionCleaner(nodeEvent.node()), 0, 5,
                            TimeUnit.MILLISECONDS);

                    NodeServices nodeServices = nodeServicesMap.put(nodeEvent.node(), new NodeServices(healthCheck, connectionCleaner));
                    if (nodeServices != null) {
                        nodeServices.connectionCleaner().cancel(true);

                        if (nodeServices.healthCheck() != null) {
                            nodeServices.healthCheck().cancel(true);
                        }
                    }
                }
            } else if (nodeEvent instanceof NodeRemovedEvent) {
                NodeServices nodeServices = nodeServicesMap.remove(nodeEvent.node());

                // Stop Connection Cleaner
                nodeServices.connectionCleaner().cancel(true);

                // If HealthCheck is not null then stop it.
                if (nodeServices.healthCheck() != null) {
                    nodeServices.healthCheck().cancel(true);
                }
            } else if (nodeEvent instanceof NodeIdleEvent) {
                // Don't do anything if Node went into IDLE state.
            } else if (nodeEvent instanceof NodeOfflineEvent) {
                // Shutdown Connection Cleaner and close all active connections.
                nodeServicesMap.get(nodeEvent.node()).connectionCleaner().cancel(true);
            }
        }
    }
}
