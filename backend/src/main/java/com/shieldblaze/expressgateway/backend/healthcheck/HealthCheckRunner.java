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
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineTask;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import lombok.extern.log4j.Log4j2;

/**
 * {@link HealthCheckService} executes {@link HealthCheck} for
 * getting the latest {@link Health} of a {@link Node}.
 */
@Log4j2
final class HealthCheckRunner implements Runnable {

    private final Node node;
    private final EventStream eventStream;

    @NonNull
    HealthCheckRunner(Node node, EventStream eventStream) {
        this.node = node;
        this.eventStream = eventStream;
    }

    @Override
    public void run() {
        try {
            // If Node is manually marked as offline, then don't run Health Check.
            if (node.state() == State.MANUAL_OFFLINE) {
                return;
            }

            HealthCheck hc = node.healthCheck();
            if (hc == null) {
                return;
            }

            final Health oldHealth = node.health(); // Store old Health
            hc.run();                               // Run a fresh Health Check

            /*
             * > If new Health is GOOD and old Health is not GOOD then update 'ONLINE' state in Node.
             * > If new Health is MEDIUM and old Health is not MEDIUM then update 'IDLE' state in Node.
             * > If new Health is BAD and old Health is not BAD then update 'OFFLINE' state in Node.
             */
            if (node.health() == Health.GOOD && oldHealth != Health.GOOD) {
                node.state(State.ONLINE);
                eventStream.publish(new NodeOnlineTask(node));
            } else if (node.health() == Health.MEDIUM && oldHealth != Health.MEDIUM) {
                node.state(State.IDLE);
                eventStream.publish(new NodeIdleTask(node));
            } else if (node.health() == Health.BAD && oldHealth != Health.BAD) {
                node.state(State.OFFLINE);
                node.drainConnections();
                eventStream.publish(new NodeOfflineTask(node));
            }
        } catch (Exception ex) {
            // Catch all exceptions to prevent the ScheduledExecutorService from
            // silently stopping this task. Uncaught exceptions in scheduled tasks
            // cause the task to be de-scheduled without any notification.
            log.error("Health check failed for node: {}", node, ex);
        }
    }
}
