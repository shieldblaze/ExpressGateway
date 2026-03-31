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
import com.shieldblaze.expressgateway.backend.exceptions.NodeNotFoundException;
import com.shieldblaze.expressgateway.backend.exceptions.StacklessException;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.concurrent.task.SyncTask;
import com.shieldblaze.expressgateway.configuration.healthcheck.HealthCheckConfiguration;
import com.shieldblaze.expressgateway.healthcheck.Health;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import lombok.ToString;

import java.io.Closeable;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.utils.ObjectUtils.nonNull;

/**
 * <p> {@link HealthCheckService} performs {@link HealthCheck} operation to
 * check {@link Health} of {@link Node}. It uses {@link ScheduledExecutorService}
 * to execute tasks. </p>
 *
 * <p> {@link #close()} must be called if this HealthCheckService is not going to be used. </p>
 */
@ToString(onlyExplicitlyIncluded = true)
public final class HealthCheckService implements com.shieldblaze.expressgateway.backend.healthcheck.HealthCheck {

    private final Map<Node, ScheduledFuture<?>> nodeMap = new ConcurrentHashMap<>();

    @ToString.Include
    private final HealthCheckConfiguration config;
    private final EventStream eventStream;
    private final ScheduledExecutorService executors;

    public HealthCheckService(HealthCheckConfiguration config, EventStream eventStream) {
        this.config = nonNull(config, HealthCheckConfiguration.class);
        this.eventStream = nonNull(eventStream, EventStream.class);
        executors = Executors.newScheduledThreadPool(config.workers());
    }

    @Override
    @NonNull
    public SyncTask<Boolean> add(Node node) {
        // HC-02: Add random jitter to the initial delay to prevent thundering herd.
        long initialDelayMs = ThreadLocalRandom.current().nextLong(config.timeInterval() * 1000L);
        ScheduledFuture<?> future = executors.scheduleAtFixedRate(new HealthCheckRunner(node, eventStream),
                initialDelayMs, config.timeInterval() * 1000L, TimeUnit.MILLISECONDS);

        // Use putIfAbsent to prevent TOCTOU race: two concurrent add() calls for
        // the same node could both pass a containsKey check and both schedule tasks,
        // leaking the first ScheduledFuture.
        ScheduledFuture<?> existing = nodeMap.putIfAbsent(node, future);
        if (existing != null) {
            // Another thread already registered this node — cancel our duplicate task.
            future.cancel(true);
            return SyncTask.of(new StacklessException("HealthCheck is already enabled for this Node: " + node));
        }
        return SyncTask.of(true);
    }

    /**
     * Remove an existing {@link Node} from the HealthCheckService.
     *
     * @throws NullPointerException If this node was not found.
     */
    @Override
    @NonNull
    public SyncTask<Boolean> remove(Node node) {
        ScheduledFuture<?> scheduledFuture = nodeMap.remove(node);
        if (scheduledFuture == null) {
            throw new NodeNotFoundException("HealthCheck is not enabled for this Node: " + node);
        }
        scheduledFuture.cancel(true);
        return SyncTask.of(true);
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
