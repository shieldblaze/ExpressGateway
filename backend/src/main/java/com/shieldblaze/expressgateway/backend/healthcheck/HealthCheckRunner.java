/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import com.shieldblaze.expressgateway.backend.events.node.NodeIdleEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOfflineEvent;
import com.shieldblaze.expressgateway.backend.events.node.NodeOnlineEvent;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventPublisher;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

/**
 * {@link HealthCheckService} executes {@link HealthCheck} for
 * getting the latest {@link Health} of a {@link Node}.
 */
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
        final Health oldHealth = node.health(); // Store old Health
        node.healthCheck().run();               // Run a fresh Health Check

        /*
         * > If new Health is GOOD and old Health is not GOOD then update 'ONLINE' state in Node.
         * > If new Health is MEDIUM and old Health is not MEDIUM then update 'IDLE' state in Node.
         * > If new Health is BAD and old Health is not BAD then update 'OFFLINE' state in Node.
         */
        if (node.health() == Health.GOOD && oldHealth != Health.GOOD) {
            node.state(State.ONLINE);
            eventStream.publish(new NodeOnlineEvent(node));
        } else if (node.health() == Health.MEDIUM && oldHealth != Health.MEDIUM) {
            node.state(State.IDLE);
            eventStream.publish(new NodeIdleEvent(node));
        } else if (node.health() == Health.BAD && oldHealth != Health.BAD) {
            node.state(State.OFFLINE);
            eventStream.publish(new NodeOfflineEvent(node));
        }
    }
}
