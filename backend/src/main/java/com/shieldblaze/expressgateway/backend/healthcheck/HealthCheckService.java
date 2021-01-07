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
package com.shieldblaze.expressgateway.backend.healthcheck;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventPublisher;
import com.shieldblaze.expressgateway.configuration.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;

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
public final class HealthCheckService {

    private final Map<Node, ScheduledFuture<?>> nodeMap = new ConcurrentHashMap<>();

    private final HealthCheckConfiguration configuration;
    private final EventPublisher eventPublisher;
    private final ScheduledExecutorService EXECUTORS;

    @NonNull
    public HealthCheckService(HealthCheckConfiguration configuration, EventPublisher eventPublisher) {
        this.configuration = configuration;
        this.eventPublisher = eventPublisher;
        EXECUTORS = Executors.newScheduledThreadPool(configuration.workers());
    }

    @NonNull
    public void add(Node node) {
        if (node.healthCheck() == null) {
            throw new IllegalArgumentException("HealthCheck is not enabled for this node.");
        }

        int interval = configuration.timeInterval();
        ScheduledFuture<?> scheduledFuture = EXECUTORS.scheduleAtFixedRate(new HealthCheckRunner(node, eventPublisher), interval, interval, TimeUnit.MILLISECONDS);
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

    public void shutdown() {
        nodeMap.forEach((node, scheduledFuture) -> scheduledFuture.cancel(true));
        nodeMap.clear();
        EXECUTORS.shutdown();
    }
}
