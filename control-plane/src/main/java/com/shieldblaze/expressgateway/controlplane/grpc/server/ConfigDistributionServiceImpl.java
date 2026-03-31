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

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.google.protobuf.ByteString;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDelta;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigAck;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigDistributionServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigFetchRequest;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigFetchResponse;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigNack;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigRequest;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigResponse;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigSubscription;
import com.shieldblaze.expressgateway.controlplane.v1.Resource;
import io.grpc.Status;
import io.grpc.stub.ServerCallStreamObserver;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.io.IOException;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

/**
 * gRPC service implementation for Aggregated Discovery Service (ADS) style
 * configuration distribution.
 *
 * <p>Manages per-node config streams. Nodes subscribe via {@code StreamConfig}
 * to receive push-based config updates. The control plane tracks each node's
 * response observer and can push deltas at any time via {@link #pushToNode}
 * or {@link #pushToAllNodes}.</p>
 *
 * <p>Thread safety: each {@link StreamObserver} stored in {@code nodeStreams}
 * is wrapped in a {@link SynchronizedObserver} that serializes all writes
 * through an intrinsic lock. This is required because gRPC StreamObservers
 * are NOT thread-safe: concurrent {@code onNext()} calls from the gRPC
 * request thread (subscribe) and the batcher flush thread (push) would
 * corrupt HTTP/2 frames.</p>
 */
@Log4j2
public final class ConfigDistributionServiceImpl
        extends ConfigDistributionServiceGrpc.ConfigDistributionServiceImplBase {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    static {
        MAPPER.findAndRegisterModules();
    }

    private final NodeRegistry registry;
    private final AtomicReference<ConfigDistributor> distributorRef;
    private final String controlPlaneId;
    private final KVStore kvStore;
    private final AtomicLong nonceCounter = new AtomicLong(0);

    /** Default maximum number of pending (buffered) pushes per node. */
    private static final int DEFAULT_MAX_PENDING_PUSHES = 100;

    /**
     * Active config streams keyed by node ID. Each entry holds a
     * {@link BackpressureAwareObserver} that wraps the server-side
     * {@link ServerCallStreamObserver} with proper gRPC flow control.
     */
    private final ConcurrentHashMap<String, BackpressureAwareObserver<ConfigResponse>> nodeStreams = new ConcurrentHashMap<>();

    /** Maximum number of pending pushes per node stream. Configurable. */
    private final int maxPendingPushes;

    /**
     * gRPC backpressure-aware wrapper around a {@link ServerCallStreamObserver}.
     *
     * <p>Instead of blindly calling {@code onNext()} (which can overwhelm the transport
     * and cause frame corruption under high push rates), this wrapper:
     * <ul>
     *   <li>Checks {@link ServerCallStreamObserver#isReady()} before each write.</li>
     *   <li>If the transport is not ready, buffers the message in a bounded
     *       per-node queue.</li>
     *   <li>Installs an {@link ServerCallStreamObserver#setOnReadyHandler(Runnable) onReadyHandler}
     *       that drains the buffer when the transport becomes writable.</li>
     *   <li>If the buffer is full, drops the oldest message and logs a warning.</li>
     * </ul>
     *
     * <p>All methods are synchronized because gRPC's StreamObserver contract
     * prohibits concurrent calls to {@code onNext/onError/onCompleted}, and
     * the onReadyHandler callback runs on a gRPC transport thread that is
     * different from the caller's thread.</p>
     *
     * @param <T> the response message type
     */
    static final class BackpressureAwareObserver<T> {
        private final ServerCallStreamObserver<T> delegate;
        private final Deque<T> buffer;
        private final int maxBuffer;
        private final String nodeId;
        private volatile boolean completed;

        BackpressureAwareObserver(ServerCallStreamObserver<T> delegate, int maxBuffer, String nodeId) {
            this.delegate = delegate;
            this.buffer = new ArrayDeque<>();
            this.maxBuffer = maxBuffer;
            this.nodeId = nodeId;
            this.completed = false;

            // Install the onReady handler to drain the buffer when the transport
            // becomes writable. This callback is invoked by gRPC's transport thread.
            delegate.setOnReadyHandler(this::drainBuffer);
        }

        synchronized void onNext(T value) {
            if (completed) {
                return;
            }
            if (delegate.isReady()) {
                // Drain any buffered messages first (FIFO order)
                drainBufferLocked();
                if (delegate.isReady()) {
                    delegate.onNext(value);
                } else {
                    enqueue(value);
                }
            } else {
                enqueue(value);
            }
        }

        synchronized void onError(Throwable t) {
            if (completed) {
                return;
            }
            completed = true;
            buffer.clear();
            delegate.onError(t);
        }

        synchronized void onCompleted() {
            if (completed) {
                return;
            }
            completed = true;
            buffer.clear();
            delegate.onCompleted();
        }

        private void enqueue(T value) {
            if (buffer.size() >= maxBuffer) {
                T dropped = buffer.pollFirst();
                if (dropped != null) {
                    log.warn("Backpressure buffer full for node {} (max={}), dropping oldest message",
                            nodeId, maxBuffer);
                }
            }
            buffer.addLast(value);
        }

        private synchronized void drainBuffer() {
            drainBufferLocked();
        }

        private void drainBufferLocked() {
            while (!buffer.isEmpty() && delegate.isReady() && !completed) {
                T msg = buffer.pollFirst();
                if (msg != null) {
                    delegate.onNext(msg);
                }
            }
        }

        int pendingCount() {
            return buffer.size();
        }
    }

    /**
     * @param registry         the node registry; must not be null
     * @param distributor      the config distributor for computing deltas; must not be null
     * @param controlPlaneId   unique identifier for this control plane instance; must not be null
     * @param kvStore          the KV store backend for loading full snapshots; must not be null
     * @param maxPendingPushes maximum number of buffered messages per node stream; must be >= 1
     */
    public ConfigDistributionServiceImpl(NodeRegistry registry, ConfigDistributor distributor,
                                         String controlPlaneId, KVStore kvStore, int maxPendingPushes) {
        this.registry = Objects.requireNonNull(registry, "registry");
        this.distributorRef = new AtomicReference<>(Objects.requireNonNull(distributor, "distributor"));
        this.controlPlaneId = Objects.requireNonNull(controlPlaneId, "controlPlaneId");
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        if (maxPendingPushes < 1) {
            throw new IllegalArgumentException("maxPendingPushes must be >= 1, got: " + maxPendingPushes);
        }
        this.maxPendingPushes = maxPendingPushes;
    }

    /**
     * @param registry       the node registry; must not be null
     * @param distributor    the config distributor for computing deltas; must not be null
     * @param controlPlaneId unique identifier for this control plane instance; must not be null
     * @param kvStore        the KV store backend for loading full snapshots; must not be null
     */
    public ConfigDistributionServiceImpl(NodeRegistry registry, ConfigDistributor distributor,
                                         String controlPlaneId, KVStore kvStore) {
        this(registry, distributor, controlPlaneId, kvStore, DEFAULT_MAX_PENDING_PUSHES);
    }

    /**
     * Constructs the service with a deferred distributor and configurable buffer size.
     *
     * @param registry         the node registry; must not be null
     * @param controlPlaneId   unique identifier for this control plane instance; must not be null
     * @param kvStore          the KV store backend for loading full snapshots; must not be null
     * @param maxPendingPushes maximum number of buffered messages per node stream; must be >= 1
     */
    public ConfigDistributionServiceImpl(NodeRegistry registry, String controlPlaneId,
                                         KVStore kvStore, int maxPendingPushes) {
        this.registry = Objects.requireNonNull(registry, "registry");
        this.distributorRef = new AtomicReference<>(null); // set later via setDistributor()
        this.controlPlaneId = Objects.requireNonNull(controlPlaneId, "controlPlaneId");
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        if (maxPendingPushes < 1) {
            throw new IllegalArgumentException("maxPendingPushes must be >= 1, got: " + maxPendingPushes);
        }
        this.maxPendingPushes = maxPendingPushes;
    }

    /**
     * Constructs the service with a deferred distributor and default buffer size.
     * Use this constructor when the {@link ConfigDistributor} cannot be provided at
     * construction time due to circular wiring (the distributor's fan-out callback
     * needs this service, and this service needs the distributor).
     *
     * <p><b>Important:</b> {@link #setDistributor(ConfigDistributor)} must be called
     * before the gRPC server starts accepting requests. Any RPC that arrives before
     * the distributor is set will fail with an {@code UNAVAILABLE} error.</p>
     *
     * @param registry       the node registry; must not be null
     * @param controlPlaneId unique identifier for this control plane instance; must not be null
     * @param kvStore        the KV store backend for loading full snapshots; must not be null
     */
    public ConfigDistributionServiceImpl(NodeRegistry registry, String controlPlaneId, KVStore kvStore) {
        this(registry, controlPlaneId, kvStore, DEFAULT_MAX_PENDING_PUSHES);
    }

    /**
     * Sets the config distributor after construction. This exists solely to break the
     * circular dependency between the distributor and this service during wiring.
     *
     * <p>Must be called exactly once, before the gRPC server starts, and must not be null.
     * Uses {@link AtomicReference#compareAndSet} to eliminate the check-then-act race
     * that existed with the previous volatile + null-check pattern.</p>
     *
     * @param distributor the config distributor; must not be null
     * @throws IllegalStateException if a distributor has already been set
     */
    public void setDistributor(ConfigDistributor distributor) {
        Objects.requireNonNull(distributor, "distributor");
        if (!distributorRef.compareAndSet(null, distributor)) {
            throw new IllegalStateException("Distributor has already been set");
        }
    }

    /**
     * Bidirectional ADS stream. Nodes send subscriptions and ack/nack responses;
     * the control plane pushes config updates as they become available.
     *
     * <p>The response observer is cast to {@link ServerCallStreamObserver} to enable
     * gRPC flow control via {@code isReady()} / {@code setOnReadyHandler()}. The
     * resulting {@link BackpressureAwareObserver} buffers up to {@code maxPendingPushes}
     * messages when the transport is not ready, and drains the buffer when it becomes
     * writable. This prevents unbounded memory growth and HTTP/2 frame corruption
     * under high push rates.</p>
     */
    @Override
    public StreamObserver<ConfigRequest> streamConfig(StreamObserver<ConfigResponse> responseObserver) {
        // gRPC guarantees that the observer passed to a server streaming / bidi method
        // is a ServerCallStreamObserver. Cast to access flow control primitives.
        ServerCallStreamObserver<ConfigResponse> serverObserver =
                (ServerCallStreamObserver<ConfigResponse>) responseObserver;

        // nodeId is not known yet (first message hasn't arrived), so pass a placeholder.
        // The actual nodeId will be set once the first ConfigRequest arrives.
        // We use "unknown" here because the observer must be created before the first message.
        BackpressureAwareObserver<ConfigResponse> bpObserver =
                new BackpressureAwareObserver<>(serverObserver, maxPendingPushes, "pending");

        return new StreamObserver<>() {

            private volatile String nodeId;

            @Override
            public void onNext(ConfigRequest request) {
                String requestNodeId = request.getNodeId();
                String sessionToken = request.getSessionToken();

                if (!registry.validateSession(requestNodeId, sessionToken)) {
                    log.warn("Config stream rejected: invalid session for node {}", requestNodeId);
                    bpObserver.onError(Status.UNAUTHENTICATED
                            .withDescription("Invalid session token for node: " + requestNodeId)
                            .asRuntimeException());
                    return;
                }

                // Track the node ID for cleanup on disconnect
                this.nodeId = requestNodeId;

                switch (request.getRequestTypeCase()) {
                    case SUBSCRIBE -> handleSubscribe(requestNodeId, request.getSubscribe(), bpObserver);
                    case ACK -> handleAck(requestNodeId, request.getAck());
                    case NACK -> handleNack(requestNodeId, request.getNack());
                    case REQUESTTYPE_NOT_SET -> {
                        log.warn("ConfigRequest from node {} has no request_type set", requestNodeId);
                        bpObserver.onError(Status.INVALID_ARGUMENT
                                .withDescription("request_type not set")
                                .asRuntimeException());
                    }
                }
            }

            @Override
            public void onError(Throwable t) {
                String id = this.nodeId;
                if (id != null) {
                    removeStream(id);
                    log.warn("Config stream error for node {}: {}", id, t.getMessage());
                } else {
                    log.warn("Config stream error from unknown node: {}", t.getMessage());
                }
            }

            @Override
            public void onCompleted() {
                String id = this.nodeId;
                if (id != null) {
                    removeStream(id);
                    log.info("Config stream completed for node {}", id);
                }
                bpObserver.onCompleted();
            }
        };
    }

    /**
     * Handle a ConfigSubscription: register the node's stream and push the initial
     * config delta based on the node's current version.
     */
    private void handleSubscribe(String nodeId, ConfigSubscription subscription,
                                 BackpressureAwareObserver<ConfigResponse> observer) {
        log.info("Node {} subscribing to config type_url={}, version={}",
                nodeId, subscription.getTypeUrl(), subscription.getVersion());

        ConfigDistributor distributor = distributorRef.get();
        if (distributor == null) {
            log.error("Distributor not set; cannot serve subscribe for node {}", nodeId);
            observer.onError(Status.UNAVAILABLE
                    .withDescription("Control plane not yet initialized")
                    .asRuntimeException());
            return;
        }

        // Complete any existing stream for this node before registering the new one
        BackpressureAwareObserver<ConfigResponse> oldStream = nodeStreams.put(nodeId, observer);
        if (oldStream != null) {
            log.info("Replacing existing config stream for node {}", nodeId);
            try {
                oldStream.onCompleted();
            } catch (Exception e) {
                log.debug("Error completing old stream for node {}: {}", nodeId, e.getMessage());
            }
        }

        // Compute the initial delta from the node's last applied version
        Optional<DataPlaneNode> nodeOpt = registry.get(nodeId);
        if (nodeOpt.isEmpty()) {
            log.warn("Subscribe from unregistered node: {}, removing stream", nodeId);
            nodeStreams.remove(nodeId);
            observer.onError(Status.NOT_FOUND
                    .withDescription("Node not registered: " + nodeId)
                    .asRuntimeException());
            return;
        }

        DataPlaneNode node = nodeOpt.get();
        ConfigDelta delta = distributor.computeNodeDelta(node.appliedConfigVersion());

        if (delta == null) {
            // Node is too far behind (journal compacted past its version) -- load the
            // entire current config state from the KV store and send a full snapshot.
            log.info("Node {} requires full snapshot resync", nodeId);
            try {
                List<KVEntry> allEntries = kvStore.listRecursive("/expressgateway/config");
                List<Resource> resources = new ArrayList<>();
                for (KVEntry entry : allEntries) {
                    try {
                        ConfigResource cr = MAPPER.readValue(entry.value(), ConfigResource.class);
                        resources.add(toProtoResource(cr));
                    } catch (IOException e) {
                        log.warn("Failed to deserialize config resource at key={} during full snapshot",
                                entry.key(), e);
                    }
                }
                ConfigResponse response = ConfigResponse.newBuilder()
                        .setTypeUrl(subscription.getTypeUrl())
                        .setVersion(String.valueOf(distributor.currentRevision()))
                        .setNonce(generateNonce())
                        .addAllResources(resources)
                        .setIsFullSnapshot(true)
                        .setControlPlaneId(controlPlaneId)
                        .build();
                observer.onNext(response);
            } catch (KVStoreException e) {
                log.error("Failed to load config resources for full snapshot to node {}", nodeId, e);
                observer.onError(Status.INTERNAL
                        .withDescription("Failed to load full config snapshot")
                        .asRuntimeException());
            }
            return;
        }

        if (delta.isEmpty()) {
            log.debug("Node {} is already up to date at revision {}", nodeId, delta.toRevision());
            return;
        }

        // Convert the delta to a ConfigResponse and push
        ConfigResponse response = buildConfigResponse(delta, subscription.getTypeUrl());
        observer.onNext(response);
    }

    /**
     * Remove a node's stream from the active streams map and complete the observer.
     * Safe to call multiple times for the same node ID.
     *
     * @param nodeId the node whose stream should be removed
     */
    public void removeStream(String nodeId) {
        BackpressureAwareObserver<ConfigResponse> removed = nodeStreams.remove(nodeId);
        if (removed != null) {
            try {
                removed.onCompleted();
            } catch (Exception e) {
                log.debug("Error completing stream for node {} during removal: {}", nodeId, e.getMessage());
            }
        }
    }

    /**
     * Handle a ConfigAck: the node successfully applied a config version.
     * Update the node's applied config version in the registry.
     */
    private void handleAck(String nodeId, ConfigAck ack) {
        log.debug("Node {} ACKed config type_url={}, version={}, nonce={}",
                nodeId, ack.getTypeUrl(), ack.getVersion(), ack.getResponseNonce());

        Optional<DataPlaneNode> nodeOpt = registry.get(nodeId);
        if (nodeOpt.isEmpty()) {
            log.warn("ACK from unregistered node: {}", nodeId);
            return;
        }

        DataPlaneNode node = nodeOpt.get();
        try {
            long version = Long.parseLong(ack.getVersion());
            // Update the node's applied version by recording a heartbeat with the new version.
            // This preserves existing metrics while advancing the config version.
            node.recordHeartbeat(version, node.activeConnections(), node.cpuUtilization(), node.memoryUtilization());
        } catch (NumberFormatException e) {
            log.warn("Node {} sent non-numeric config version in ACK: {}", nodeId, ack.getVersion());
        }
    }

    /**
     * Handle a ConfigNack: the node rejected a config version.
     * Log the error but do NOT retry immediately to avoid push storms.
     */
    private void handleNack(String nodeId, ConfigNack nack) {
        log.warn("Node {} NACKed config type_url={}, version={}, nonce={}, error=[code={}, message={}, failed_resources={}]",
                nodeId, nack.getTypeUrl(), nack.getVersion(), nack.getResponseNonce(),
                nack.hasError() ? nack.getError().getCode() : "N/A",
                nack.hasError() ? nack.getError().getMessage() : "N/A",
                nack.hasError() ? nack.getError().getFailedResourcesList() : List.of());
    }

    /**
     * Push a config delta to a specific node.
     *
     * <p>Looks up the node's active stream observer and sends a {@link ConfigResponse}
     * containing the delta's mutations. If the node has no active stream, the push
     * is silently dropped -- the node will receive the delta on its next subscription.</p>
     *
     * <p>Thread-safe: writes are serialized through the {@link SynchronizedObserver}.</p>
     *
     * @param nodeId the target node ID
     * @param delta  the config delta to push
     */
    public void pushToNode(String nodeId, ConfigDelta delta) {
        BackpressureAwareObserver<ConfigResponse> observer = nodeStreams.get(nodeId);
        if (observer == null) {
            log.debug("No active config stream for node {}, skipping push", nodeId);
            return;
        }

        try {
            ConfigResponse response = buildConfigResponse(delta, "");
            observer.onNext(response);
            log.debug("Pushed config delta to node {} (revision {} -> {}, pending={})",
                    nodeId, delta.fromRevision(), delta.toRevision(), observer.pendingCount());
        } catch (Exception e) {
            log.error("Failed to push config to node {}", nodeId, e);
            nodeStreams.remove(nodeId);
        }
    }

    /**
     * Broadcast a config delta to all nodes with active config streams.
     *
     * @param delta the config delta to broadcast
     */
    public void pushToAllNodes(ConfigDelta delta) {
        ConfigResponse response = buildConfigResponse(delta, "");
        for (var entry : nodeStreams.entrySet()) {
            String nodeId = entry.getKey();
            BackpressureAwareObserver<ConfigResponse> observer = entry.getValue();
            try {
                observer.onNext(response);
                log.debug("Broadcast config delta to node {} (revision {} -> {}, pending={})",
                        nodeId, delta.fromRevision(), delta.toRevision(), observer.pendingCount());
            } catch (Exception e) {
                log.error("Failed to broadcast config to node {}", nodeId, e);
                nodeStreams.remove(nodeId);
            }
        }
    }

    /**
     * Unary fetch for nodes that need a one-shot config pull.
     */
    @Override
    public void fetchConfig(ConfigFetchRequest request, StreamObserver<ConfigFetchResponse> responseObserver) {
        String nodeId = request.getNodeId();
        String sessionToken = request.getSessionToken();

        if (!registry.validateSession(nodeId, sessionToken)) {
            responseObserver.onError(Status.UNAUTHENTICATED
                    .withDescription("Invalid session token for node: " + nodeId)
                    .asRuntimeException());
            return;
        }

        Optional<DataPlaneNode> nodeOpt = registry.get(nodeId);
        if (nodeOpt.isEmpty()) {
            responseObserver.onError(Status.NOT_FOUND
                    .withDescription("Node not registered: " + nodeId)
                    .asRuntimeException());
            return;
        }

        DataPlaneNode node = nodeOpt.get();
        ConfigDistributor distributor = distributorRef.get();
        if (distributor == null) {
            responseObserver.onError(Status.UNAVAILABLE
                    .withDescription("Control plane not yet initialized")
                    .asRuntimeException());
            return;
        }

        ConfigDelta delta = distributor.computeNodeDelta(node.appliedConfigVersion());

        ConfigFetchResponse.Builder responseBuilder = ConfigFetchResponse.newBuilder()
                .setTypeUrl(request.getTypeUrl())
                .setVersion(String.valueOf(distributor.currentRevision()));

        if (delta != null && !delta.isEmpty()) {
            for (ConfigMutation mutation : delta.mutations()) {
                switch (mutation) {
                    case ConfigMutation.Upsert upsert -> responseBuilder.addResources(toProtoResource(upsert.resource()));
                    case ConfigMutation.Delete delete -> responseBuilder.addRemovedResources(delete.resourceId().toPath());
                }
            }
        }

        responseObserver.onNext(responseBuilder.build());
        responseObserver.onCompleted();
    }

    /**
     * Build a {@link ConfigResponse} from a {@link ConfigDelta}.
     *
     * <p>Upsert mutations are converted to {@link Resource} messages.
     * Delete mutations are added to the {@code removed_resources} list.</p>
     */
    private ConfigResponse buildConfigResponse(ConfigDelta delta, String typeUrl) {
        List<Resource> resources = new ArrayList<>();
        List<String> removedResources = new ArrayList<>();

        for (ConfigMutation mutation : delta.mutations()) {
            switch (mutation) {
                case ConfigMutation.Upsert upsert -> resources.add(toProtoResource(upsert.resource()));
                case ConfigMutation.Delete delete -> removedResources.add(delete.resourceId().toPath());
            }
        }

        return ConfigResponse.newBuilder()
                .setTypeUrl(typeUrl)
                .setVersion(String.valueOf(delta.toRevision()))
                .setNonce(generateNonce())
                .addAllResources(resources)
                .addAllRemovedResources(removedResources)
                .setControlPlaneId(controlPlaneId)
                .build();
    }

    /**
     * Convert a {@link ConfigResource} to a proto {@link Resource} message.
     *
     * <p>The resource's spec is serialized to JSON bytes for the payload.
     * Serialization failures are propagated as unchecked exceptions rather than
     * silently sending an empty payload, which would cause the data plane node
     * to apply a broken (empty) config resource.</p>
     *
     * @throws IllegalStateException if the resource spec cannot be serialized
     */
    private Resource toProtoResource(ConfigResource resource) {
        byte[] payloadBytes;
        try {
            payloadBytes = MAPPER.writeValueAsBytes(resource.spec());
        } catch (JsonProcessingException e) {
            throw new IllegalStateException(
                    "Failed to serialize config spec for resource " + resource.id().toPath(), e);
        }

        return Resource.newBuilder()
                .setName(resource.id().name())
                .setTypeUrl(resource.kind().name())
                .setVersion(String.valueOf(resource.version()))
                .setPayload(ByteString.copyFrom(payloadBytes))
                .build();
    }

    /**
     * Generate a unique nonce for correlating config push/ack cycles.
     * Format: controlPlaneId:monotonicallyIncreasingCounter
     */
    private String generateNonce() {
        return controlPlaneId + ":" + nonceCounter.incrementAndGet();
    }

    /**
     * Returns the number of active config streams.
     * Useful for monitoring and debugging.
     */
    public int activeStreamCount() {
        return nodeStreams.size();
    }
}
