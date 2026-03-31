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
package com.shieldblaze.expressgateway.controlplane.grpc.server;

import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.v1.DeregisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.DeregisterResponse;
import com.shieldblaze.expressgateway.controlplane.v1.HeartbeatDirective;
import com.shieldblaze.expressgateway.controlplane.v1.HeartbeatResponse;
import com.shieldblaze.expressgateway.controlplane.v1.NodeHeartbeat;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import com.shieldblaze.expressgateway.controlplane.v1.NodeRegistrationServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterResponse;
import com.shieldblaze.expressgateway.controlplane.v1.Timestamp;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.security.SecureRandom;
import java.util.HexFormat;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * gRPC service implementation for data-plane node lifecycle management:
 * registration, deregistration, and heartbeat streaming.
 *
 * <p>Thread safety: this class is stateless aside from the injected {@link NodeRegistry}
 * which is itself thread-safe. Each streaming heartbeat call operates on its own
 * {@link StreamObserver} pair and does not share mutable state with other calls.</p>
 */
@Log4j2
public final class NodeRegistrationServiceImpl
        extends NodeRegistrationServiceGrpc.NodeRegistrationServiceImplBase {

    /**
     * Callback interface for heartbeat stream errors. Allows other services
     * (e.g., ConfigDistributionServiceImpl) to be notified when a node's
     * heartbeat stream fails so they can clean up associated state.
     */
    @FunctionalInterface
    public interface HeartbeatStreamErrorListener {
        /**
         * Called when a heartbeat stream encounters an error.
         *
         * @param nodeId the node whose heartbeat stream errored
         * @param error  the error that occurred
         */
        void onHeartbeatStreamError(String nodeId, Throwable error);
    }

    private static final SecureRandom SECURE_RANDOM = new SecureRandom();
    private static final int SESSION_TOKEN_BYTES = 32; // 256 bits of entropy

    private final NodeRegistry registry;
    private final String controlPlaneId;
    private final long heartbeatIntervalMs;
    private final int maxNodes;
    private final List<HeartbeatStreamErrorListener> heartbeatErrorListeners = new CopyOnWriteArrayList<>();

    /**
     * @param registry           the node registry; must not be null
     * @param controlPlaneId     unique identifier for this control plane instance; must not be null
     * @param heartbeatIntervalMs the heartbeat interval in milliseconds to advertise to nodes
     * @param maxNodes           maximum number of registered nodes; must be >= 1
     */
    public NodeRegistrationServiceImpl(NodeRegistry registry, String controlPlaneId,
                                       long heartbeatIntervalMs, int maxNodes) {
        this.registry = Objects.requireNonNull(registry, "registry");
        this.controlPlaneId = Objects.requireNonNull(controlPlaneId, "controlPlaneId");
        if (heartbeatIntervalMs <= 0) {
            throw new IllegalArgumentException("heartbeatIntervalMs must be > 0, got: " + heartbeatIntervalMs);
        }
        if (maxNodes < 1) {
            throw new IllegalArgumentException("maxNodes must be >= 1, got: " + maxNodes);
        }
        this.heartbeatIntervalMs = heartbeatIntervalMs;
        this.maxNodes = maxNodes;
    }

    /**
     * Registers a listener to be notified when a heartbeat stream errors.
     *
     * @param listener the listener; must not be null
     */
    public void addHeartbeatStreamErrorListener(HeartbeatStreamErrorListener listener) {
        heartbeatErrorListeners.add(Objects.requireNonNull(listener, "listener"));
    }

    /**
     * Register a new data-plane node with the control plane.
     *
     * <p>Validates the auth token (non-empty check for now), generates a session token,
     * and registers the node in the {@link NodeRegistry}. On duplicate node ID or other
     * registration errors, responds with {@code accepted=false}.</p>
     */
    @Override
    public void register(RegisterRequest request, StreamObserver<RegisterResponse> responseObserver) {
        try {
            NodeIdentity identity = request.getIdentity();
            if (identity == null || identity.getNodeId().isEmpty()) {
                responseObserver.onNext(RegisterResponse.newBuilder()
                        .setAccepted(false)
                        .setRejectReason("Missing or empty node identity")
                        .build());
                responseObserver.onCompleted();
                return;
            }

            // Validate auth_token -- for now just check non-empty.
            // Production should verify against an external auth service or HMAC.
            String authToken = request.getAuthToken();
            if (authToken == null || authToken.isEmpty()) {
                responseObserver.onNext(RegisterResponse.newBuilder()
                        .setAccepted(false)
                        .setRejectReason("Missing auth_token")
                        .build());
                responseObserver.onCompleted();
                return;
            }

            // Enforce maxNodes limit
            if (registry.size() >= maxNodes) {
                log.warn("Registration rejected for node {}: maxNodes limit reached ({})",
                        identity.getNodeId(), maxNodes);
                responseObserver.onNext(RegisterResponse.newBuilder()
                        .setAccepted(false)
                        .setRejectReason("Maximum number of registered nodes reached (" + maxNodes + ")")
                        .build());
                responseObserver.onCompleted();
                return;
            }

            byte[] tokenBytes = new byte[SESSION_TOKEN_BYTES];
            SECURE_RANDOM.nextBytes(tokenBytes);
            String sessionToken = HexFormat.of().formatHex(tokenBytes);
            registry.register(identity, sessionToken);

            log.info("Node {} registered successfully (cluster={}, env={})",
                    identity.getNodeId(), identity.getClusterId(), identity.getEnvironment());

            responseObserver.onNext(RegisterResponse.newBuilder()
                    .setAccepted(true)
                    .setSessionToken(sessionToken)
                    .setControlPlaneId(controlPlaneId)
                    .setHeartbeatIntervalMs(heartbeatIntervalMs)
                    .build());
            responseObserver.onCompleted();

        } catch (IllegalStateException e) {
            // Node already registered
            log.warn("Registration rejected for node: {}", e.getMessage());
            responseObserver.onNext(RegisterResponse.newBuilder()
                    .setAccepted(false)
                    .setRejectReason(e.getMessage())
                    .build());
            responseObserver.onCompleted();

        } catch (Exception e) {
            log.error("Unexpected error during registration", e);
            responseObserver.onNext(RegisterResponse.newBuilder()
                    .setAccepted(false)
                    .setRejectReason("Internal error")
                    .build());
            responseObserver.onCompleted();
        }
    }

    /**
     * Deregister a node (graceful shutdown / maintenance).
     *
     * <p>Validates the session token before proceeding. The node is removed from the
     * registry and transitioned to DISCONNECTED state.</p>
     */
    @Override
    public void deregister(DeregisterRequest request, StreamObserver<DeregisterResponse> responseObserver) {
        String nodeId = request.getNodeId();
        String sessionToken = request.getSessionToken();

        if (nodeId == null || nodeId.isEmpty()) {
            responseObserver.onError(Status.INVALID_ARGUMENT
                    .withDescription("Missing node_id")
                    .asRuntimeException());
            return;
        }

        if (!registry.validateSession(nodeId, sessionToken)) {
            responseObserver.onError(Status.UNAUTHENTICATED
                    .withDescription("Invalid session token for node: " + nodeId)
                    .asRuntimeException());
            return;
        }

        DataPlaneNode removed = registry.deregister(nodeId);
        if (removed != null) {
            log.info("Node {} deregistered (reason={})", nodeId, request.getReason());
        }

        responseObserver.onNext(DeregisterResponse.newBuilder()
                .setAcknowledged(removed != null)
                .build());
        responseObserver.onCompleted();
    }

    /**
     * Bidirectional heartbeat stream. The node sends periodic beats; the control plane
     * responds with acknowledgements or operational directives.
     *
     * <p>Each heartbeat updates the node's metrics (connections, CPU, memory) and
     * resets the missed heartbeat counter. The response carries an ACK directive.
     * Future enhancements can add reconnect or resubscribe directives based on
     * cluster-level decisions.</p>
     */
    @Override
    public StreamObserver<NodeHeartbeat> heartbeat(StreamObserver<HeartbeatResponse> responseObserver) {
        return new StreamObserver<>() {

            private volatile String trackedNodeId;

            @Override
            public void onNext(NodeHeartbeat heartbeat) {
                String nodeId = heartbeat.getNodeId();
                String sessionToken = heartbeat.getSessionToken();

                // Track the node ID for cleanup on error/completion
                this.trackedNodeId = nodeId;

                if (!registry.validateSession(nodeId, sessionToken)) {
                    log.warn("Heartbeat rejected: invalid session for node {}", nodeId);
                    responseObserver.onError(Status.UNAUTHENTICATED
                            .withDescription("Invalid session token for node: " + nodeId)
                            .asRuntimeException());
                    return;
                }

                Optional<DataPlaneNode> nodeOpt = registry.get(nodeId);
                if (nodeOpt.isEmpty()) {
                    log.warn("Heartbeat from unknown node: {}", nodeId);
                    responseObserver.onError(Status.NOT_FOUND
                            .withDescription("Node not registered: " + nodeId)
                            .asRuntimeException());
                    return;
                }

                DataPlaneNode node = nodeOpt.get();
                node.recordHeartbeat(
                        node.appliedConfigVersion(), // preserve current applied version
                        heartbeat.getActiveConnections(),
                        heartbeat.getCpuUtilization(),
                        heartbeat.getMemoryUtilization()
                );

                if (log.isDebugEnabled()) {
                    log.debug("Heartbeat from node {} (health={}, connections={}, cpu={}, mem={})",
                            nodeId, heartbeat.getHealthStatus(),
                            heartbeat.getActiveConnections(),
                            String.format("%.2f", heartbeat.getCpuUtilization()),
                            String.format("%.2f", heartbeat.getMemoryUtilization()));
                }

                HeartbeatResponse response = HeartbeatResponse.newBuilder()
                        .setTimestamp(Timestamp.newBuilder()
                                .setEpochMillis(System.currentTimeMillis())
                                .build())
                        .addDirectives(HeartbeatDirective.newBuilder()
                                .setAck(true)
                                .build())
                        .build();

                responseObserver.onNext(response);
            }

            @Override
            public void onError(Throwable t) {
                String nodeId = this.trackedNodeId;
                if (nodeId != null) {
                    log.warn("Heartbeat stream error for node {}", nodeId, t);
                    // Notify listeners so they can clean up associated state
                    // (e.g., config streams, pending pushes)
                    for (HeartbeatStreamErrorListener listener : heartbeatErrorListeners) {
                        try {
                            listener.onHeartbeatStreamError(nodeId, t);
                        } catch (Exception e) {
                            log.error("Error notifying heartbeat stream error listener for node {}", nodeId, e);
                        }
                    }
                } else {
                    log.warn("Heartbeat stream error from unknown node: {}", t.getMessage());
                }
            }

            @Override
            public void onCompleted() {
                log.info("Heartbeat stream completed");
                responseObserver.onCompleted();
            }
        };
    }
}
