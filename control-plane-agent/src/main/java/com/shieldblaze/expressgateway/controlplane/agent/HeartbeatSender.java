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
package com.shieldblaze.expressgateway.controlplane.agent;

import com.shieldblaze.expressgateway.controlplane.v1.HeartbeatResponse;
import com.shieldblaze.expressgateway.controlplane.v1.NodeHealthStatus;
import com.shieldblaze.expressgateway.controlplane.v1.NodeHeartbeat;
import com.shieldblaze.expressgateway.controlplane.v1.NodeRegistrationServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.ReconnectDirective;
import com.shieldblaze.expressgateway.controlplane.v1.Timestamp;
import io.grpc.stub.StreamObserver;
import lombok.extern.slf4j.Slf4j;

import java.io.Closeable;
import java.util.List;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.function.IntSupplier;

/**
 * Sends periodic heartbeats to the Control Plane via gRPC bidirectional stream.
 *
 * <p>Each heartbeat carries the node's health status and basic utilization metrics.
 * The control plane may respond with directives (reconnect, resubscribe) that the
 * agent must act on.</p>
 *
 * <p>Thread safety: all writes to the gRPC {@code requestObserver} are serialized
 * through {@code writeLock}. This is required because {@code sendHeartbeat()} runs
 * on the scheduler thread while gRPC callbacks fire on the gRPC executor thread;
 * gRPC's StreamObserver is NOT thread-safe.</p>
 */
@Slf4j
public final class HeartbeatSender implements Closeable {

    /**
     * Callback invoked when the control plane sends a reconnect directive,
     * indicating this node should connect to a different CP instance.
     */
    @FunctionalInterface
    public interface ReconnectCallback {
        void onReconnect(String targetAddress, String reason);
    }

    /**
     * Callback invoked when the control plane sends a resubscribe directive,
     * indicating this node should resubscribe to the specified resource types.
     */
    @FunctionalInterface
    public interface ResubscribeCallback {
        void onResubscribe(List<String> resourceTypes);
    }

    private final String nodeId;
    private final String sessionToken;
    private final long intervalMs;
    private final ScheduledExecutorService scheduler;
    private volatile StreamObserver<NodeHeartbeat> requestObserver;
    private volatile boolean running;

    private volatile ReconnectCallback reconnectCallback;
    private volatile ResubscribeCallback resubscribeCallback;
    private volatile IntSupplier activeConnectionsSupplier = () -> 0;

    /**
     * Lock that serializes all writes ({@code onNext}, {@code onCompleted}) to
     * the gRPC {@code requestObserver}. Without this, the scheduler thread and
     * the gRPC callback thread can issue concurrent {@code onNext()} calls,
     * corrupting HTTP/2 frames on the wire.
     */
    private final Object writeLock = new Object();

    public HeartbeatSender(String nodeId, String sessionToken, long intervalMs) {
        this.nodeId = nodeId;
        this.sessionToken = sessionToken;
        this.intervalMs = intervalMs;
        this.scheduler = Executors.newSingleThreadScheduledExecutor(Thread.ofVirtual()
                .name("cp-agent-heartbeat")
                .factory());
    }

    /**
     * Set the callback to invoke when the CP sends a reconnect directive.
     */
    public void setReconnectCallback(ReconnectCallback callback) {
        this.reconnectCallback = callback;
    }

    /**
     * Set the callback to invoke when the CP sends a resubscribe directive.
     */
    public void setResubscribeCallback(ResubscribeCallback callback) {
        this.resubscribeCallback = callback;
    }

    /**
     * Set a supplier for the current active connection count, reported in heartbeats.
     */
    public void setActiveConnectionsSupplier(IntSupplier supplier) {
        this.activeConnectionsSupplier = supplier;
    }

    /**
     * Start sending heartbeats using the provided gRPC stub.
     *
     * @param stub the async stub for the NodeRegistrationService
     */
    public void start(NodeRegistrationServiceGrpc.NodeRegistrationServiceStub stub) {
        running = true;

        // Open bidirectional heartbeat stream
        requestObserver = stub.heartbeat(new StreamObserver<>() {
            @Override
            public void onNext(HeartbeatResponse response) {
                // Process directives (reconnect, resubscribe, etc.)
                for (var directive : response.getDirectivesList()) {
                    if (directive.hasReconnect()) {
                        ReconnectDirective reconnect = directive.getReconnect();
                        log.info("Received reconnect directive: target={}, reason={}",
                                reconnect.getTargetAddress(), reconnect.getReason());
                        ReconnectCallback cb = reconnectCallback;
                        if (cb != null) {
                            try {
                                cb.onReconnect(reconnect.getTargetAddress(), reconnect.getReason());
                            } catch (Exception e) {
                                log.error("Reconnect callback failed", e);
                            }
                        }
                    }
                    if (directive.hasResubscribe()) {
                        List<String> types = directive.getResubscribe().getResourceTypesList();
                        log.info("Received resubscribe directive for types: {}", types);
                        ResubscribeCallback cb = resubscribeCallback;
                        if (cb != null) {
                            try {
                                cb.onResubscribe(types);
                            } catch (Exception e) {
                                log.error("Resubscribe callback failed", e);
                            }
                        }
                    }
                }
            }

            @Override
            public void onError(Throwable t) {
                log.warn("Heartbeat stream error", t);
                running = false;
            }

            @Override
            public void onCompleted() {
                log.info("Heartbeat stream completed");
                running = false;
            }
        });

        // Schedule periodic heartbeat sends
        scheduler.scheduleAtFixedRate(this::sendHeartbeat, 0, intervalMs, TimeUnit.MILLISECONDS);
    }

    private void sendHeartbeat() {
        if (!running || requestObserver == null) {
            return;
        }
        try {
            NodeHeartbeat heartbeat = NodeHeartbeat.newBuilder()
                    .setNodeId(nodeId)
                    .setSessionToken(sessionToken)
                    .setTimestamp(Timestamp.newBuilder()
                            .setEpochMillis(System.currentTimeMillis())
                            .build())
                    .setHealthStatus(NodeHealthStatus.NODE_HEALTH_HEALTHY)
                    .setActiveConnections(activeConnectionsSupplier.getAsInt())
                    .build();
            synchronized (writeLock) {
                requestObserver.onNext(heartbeat);
            }
        } catch (Exception e) {
            log.warn("Failed to send heartbeat", e);
        }
    }

    @Override
    public void close() {
        running = false;

        // Shut down the scheduler FIRST so no new sendHeartbeat() calls can
        // race with the onCompleted() below.
        scheduler.shutdown();
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
                log.warn("Heartbeat scheduler did not terminate within 5s, forced shutdown");
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }

        // Now that the scheduler is fully stopped, complete the stream.
        StreamObserver<NodeHeartbeat> observer = requestObserver;
        if (observer != null) {
            try {
                synchronized (writeLock) {
                    observer.onCompleted();
                }
            } catch (Exception _) {
                // Stream may already be closed
            }
        }
    }
}
