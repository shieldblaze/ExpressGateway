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
package com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor;

import io.grpc.Metadata;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import io.grpc.Status;
import lombok.extern.log4j.Log4j2;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * gRPC server interceptor that enforces per-node request rate limiting.
 *
 * <p>Tracks request counts per node (identified by the {@code x-node-id} metadata header)
 * using a {@link ConcurrentHashMap} of {@link AtomicInteger} counters. A background
 * scheduled task resets all counters every second. If a node exceeds the configured
 * maximum requests per second, the call is rejected with {@link Status#RESOURCE_EXHAUSTED}.</p>
 *
 * <p>Design notes:</p>
 * <ul>
 *   <li>This is a simple fixed-window counter, not a sliding window or token bucket.
 *       It is sufficient for protecting the control plane from misbehaving nodes but
 *       allows brief bursts at window boundaries (up to 2x the limit in a 1-second span).
 *       For production, consider upgrading to a token bucket or sliding window log.</li>
 *   <li>Counter reset runs on a daemon thread to avoid blocking the gRPC event loop.</li>
 *   <li>If the {@code x-node-id} header is absent, the call is allowed through without
 *       rate limiting (the auth interceptor or service layer will reject it later).</li>
 * </ul>
 */
@Log4j2
public final class RateLimitInterceptor implements ServerInterceptor {

    /**
     * Metadata key for the node ID header used to identify the source node.
     */
    static final Metadata.Key<String> NODE_ID_KEY =
            Metadata.Key.of("x-node-id", Metadata.ASCII_STRING_MARSHALLER);

    /**
     * Default maximum requests per second per node.
     */
    private static final int DEFAULT_MAX_REQUESTS_PER_SECOND = 100;

    private final int maxRequestsPerSecond;
    private final ConcurrentHashMap<String, AtomicInteger> requestCounts = new ConcurrentHashMap<>();
    private final ScheduledExecutorService resetScheduler;

    /**
     * Create a rate limiter with the default limit of {@value DEFAULT_MAX_REQUESTS_PER_SECOND}
     * requests per second per node.
     */
    public RateLimitInterceptor() {
        this(DEFAULT_MAX_REQUESTS_PER_SECOND);
    }

    /**
     * Create a rate limiter with a custom limit.
     *
     * @param maxRequestsPerSecond the maximum number of requests per second per node; must be > 0
     */
    public RateLimitInterceptor(int maxRequestsPerSecond) {
        if (maxRequestsPerSecond <= 0) {
            throw new IllegalArgumentException("maxRequestsPerSecond must be > 0, got: " + maxRequestsPerSecond);
        }
        this.maxRequestsPerSecond = maxRequestsPerSecond;

        this.resetScheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "cp-rate-limit-reset");
            t.setDaemon(true);
            return t;
        });
        this.resetScheduler.scheduleAtFixedRate(this::resetCounters, 1, 1, TimeUnit.SECONDS);
    }

    @Override
    public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
            ServerCall<ReqT, RespT> call,
            Metadata headers,
            ServerCallHandler<ReqT, RespT> next) {

        String nodeId = headers.get(NODE_ID_KEY);
        if (nodeId == null || nodeId.isEmpty()) {
            // No node ID -- cannot rate limit. Let the call through;
            // auth interceptor or service layer will handle rejection.
            return next.startCall(call, headers);
        }

        AtomicInteger counter = requestCounts.computeIfAbsent(nodeId, k -> new AtomicInteger(0));
        int currentCount = counter.incrementAndGet();

        if (currentCount > maxRequestsPerSecond) {
            log.warn("Rate limit exceeded for node {} ({}/{}), rejecting {}",
                    nodeId, currentCount, maxRequestsPerSecond,
                    call.getMethodDescriptor().getBareMethodName());
            call.close(
                    Status.RESOURCE_EXHAUSTED.withDescription(
                            "Rate limit exceeded: " + maxRequestsPerSecond + " requests/second"),
                    new Metadata());
            return new ServerCall.Listener<>() {};
        }

        return next.startCall(call, headers);
    }

    /**
     * Reset all per-node request counters. Called every second by the scheduled task.
     *
     * <p>Uses {@link AtomicInteger#set(int)} which is a volatile write, guaranteeing
     * visibility to all threads on the next read. We also prune entries for nodes that
     * had zero requests in the last window to prevent unbounded map growth from
     * nodes that registered once and never sent traffic again.</p>
     */
    private void resetCounters() {
        requestCounts.forEach((nodeId, counter) -> {
            if (counter.get() == 0) {
                // Prune idle entries to prevent map growth.
                // Use remove(key, value) for safe concurrent removal.
                requestCounts.remove(nodeId, counter);
            } else {
                counter.set(0);
            }
        });
    }

    /**
     * Shutdown the background reset scheduler. Call on application shutdown.
     */
    public void shutdown() {
        resetScheduler.shutdown();
        try {
            if (!resetScheduler.awaitTermination(2, TimeUnit.SECONDS)) {
                resetScheduler.shutdownNow();
            }
        } catch (InterruptedException e) {
            resetScheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }
    }
}
