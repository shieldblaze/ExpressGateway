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
package com.shieldblaze.expressgateway.core.controller;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.events.node.NodeAddedEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeRemovedEvent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

final class NodeHandler {

    private static final Logger logger = LogManager.getLogger(NodeHandler.class);

    private final Map<Node, NodeHolder> nodeHolderMap = new ConcurrentHashMap<>();
    private final Controller controller;

    public NodeHandler(Controller controller) {
        this.controller = controller;
    }

    void handleEvent(NodeEvent event) {
        if (event instanceof NodeAddedEvent || event instanceof NodeOnlineEvent) {
            nodeAdded(event.node());
        } else if (event instanceof NodeRemovedEvent) {
            nodeRemoved(event);
        } else if (event instanceof NodeOfflineEvent || event instanceof NodeIdleEvent) {
            // Don't do anything if Node went into IDLE state.
        }
    }

    /**
     * {@link NodeAddedEvent}: Once a node is added into a cluster, it'll pass this event in stream.
     * When this event is received then we'll spin up HealthCheck and ConnectionCleaner service.
     */
    private void nodeAdded(Node node) {
        ScheduledFuture<?> healthCheck = null;
        ScheduledFuture<?> connectionCleaner;

        // If Health Check is associated with the node then we'll run it.
        if (node.healthCheck() != null) {
            long interval = controller.configuration.healthCheckIntervalMilliseconds();
            healthCheck = controller.loopWorkers.scheduleWithFixedDelay(new HealthCheckService(node, controller.eventPublisher),
                    0, interval, TimeUnit.MILLISECONDS);
        }

        long interval = controller.configuration.deadConnectionCleanIntervalMilliseconds();
        connectionCleaner = controller.loopWorkers.scheduleWithFixedDelay(new DeadConnectionCleaner(node), 0, interval, TimeUnit.MILLISECONDS);

        NodeHolder nodeHolder = nodeHolderMap.put(node, new NodeHolder(healthCheck, connectionCleaner));

        // If NodeHolder is not null it means the instance came online from a different state.
        // In this case, we'll cancel old ConnectionCleaner and HealthCheck service.
        if (nodeHolder != null) {
            nodeHolder.deadConnectionCleaner().cancel(true);

            if (nodeHolder.healthCheck() != null) {
                nodeHolder.healthCheck().cancel(true);
            }
        }
    }

    private void nodeRemoved(NodeEvent event) {
        NodeHolder nodeHolder = nodeHolderMap.remove(event.node());

        if (nodeHolder == null) {
            // This should never happen.
            logger.error("NodeHandler was 'null', Event: {}", event);
            return;
        }

        // Stop Connection Cleaner
        nodeHolder.deadConnectionCleaner().cancel(true);

        // If HealthCheck is not null then stop it.
        if (nodeHolder.healthCheck() != null) {
            nodeHolder.healthCheck().cancel(true);
        }
    }
}
