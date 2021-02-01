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
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

import java.io.Closeable;
import java.io.IOException;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * {@link HealthCheckService} performs {@link HealthCheck} operation to
 * check {@link Health} of {@link Node}.
 */
public final class HealthCheckService implements Closeable {

    private final Map<Node, ScheduledFuture<?>> nodeMap = new ConcurrentHashMap<>();

    private final HealthCheckConfiguration configuration;
    private final EventStream eventStream;
    private final ScheduledExecutorService executors;

    @NonNull
    public HealthCheckService(HealthCheckConfiguration configuration, EventStream eventStream) {
        this.configuration = configuration;
        this.eventStream = eventStream;
        executors = Executors.newScheduledThreadPool(configuration.workers());
    }

    @NonNull
    public void add(Node node) {
        if (node.healthCheck() == null) {
            throw new IllegalArgumentException("HealthCheck is not enabled for this node.");
        }

        ScheduledFuture<?> scheduledFuture = executors.scheduleAtFixedRate(new HealthCheckRunner(node, eventStream),
                0, configuration.timeInterval(), TimeUnit.SECONDS);
        nodeMap.put(node, scheduledFuture);
    }

    @NonNull
    public void remove(Node node) {
        if (node.healthCheck() == null) {
            throw new IllegalArgumentException("HealthCheck is not enabled for this node.");
        }

        ScheduledFuture<?> scheduledFuture = nodeMap.get(node);
        if (scheduledFuture != null) {
            scheduledFuture.cancel(true);
        }
    }

    @Override
    public void close() throws IOException {
        nodeMap.forEach((node, scheduledFuture) -> scheduledFuture.cancel(true));
        nodeMap.clear();
        executors.shutdown();
    }
}
