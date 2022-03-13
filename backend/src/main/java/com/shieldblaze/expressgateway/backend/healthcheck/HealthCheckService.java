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
import com.shieldblaze.expressgateway.common.annotation.InternalCall;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.io.Closeable;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * <p> {@link HealthCheckService} performs {@link HealthCheck} operation to
 * check {@link Health} of {@link Node}. It uses {@link ScheduledExecutorService}
 * to execute tasks. </p>
 *
 * <p> {@link #close()} must be called if this HealthCheckService is not going to be used. </p>
 */
public final class HealthCheckService implements Closeable {

    private final Map<Node, ScheduledFuture<?>> nodeMap = new ConcurrentHashMap<>();

    private final HealthCheckConfiguration config;
    private final EventStream eventStream;
    private final ScheduledExecutorService executors;

    public HealthCheckService(HealthCheckConfiguration config, EventStream eventStream) {
        this.config = config;
        this.eventStream = eventStream;
        executors = Executors.newScheduledThreadPool(config.workers());
    }

    /**
     * Add a new {@link Node} to the HealthCheckService.
     *
     * @throws IllegalArgumentException If HealthCheck is not enabled for this {@link Node}
     */
    @NonNull
    public void add(Node node) {
        if (nodeMap.containsKey(node)) {
            throw new IllegalArgumentException("HealthCheck is already enabled for this Node: " + node);
        }

        nodeMap.put(node, executors.scheduleAtFixedRate(new HealthCheckRunner(node, eventStream), 0, config.timeInterval(), TimeUnit.SECONDS));
    }

    /**
     * Remove a existing {@link Node} from the HealthCheckService.
     *
     * @throws IllegalArgumentException If this node was not found.
     */
    @NonNull
    public void remove(Node node) {
        ScheduledFuture<?> scheduledFuture = nodeMap.get(node);
        if (scheduledFuture != null) {
            scheduledFuture.cancel(true);
        } else {
            throw new IllegalArgumentException("Node not found in HealthCheckService");
        }
    }

    /**
     * Close this HealthCheckService and stops all running operations.
     */
    @Override
    public void close() {
        nodeMap.forEach((node, scheduledFuture) -> scheduledFuture.cancel(true));
        nodeMap.clear();
        if (executors != null) {
            executors.shutdown();
        }
    }
}
