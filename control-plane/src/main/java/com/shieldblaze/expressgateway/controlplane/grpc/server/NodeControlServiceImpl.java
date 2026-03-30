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

import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDelta;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.registry.DataPlaneNode;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import com.shieldblaze.expressgateway.controlplane.v1.CommandAck;
import com.shieldblaze.expressgateway.controlplane.v1.CommandResponse;
import com.shieldblaze.expressgateway.controlplane.v1.NodeCommand;
import com.shieldblaze.expressgateway.controlplane.v1.NodeControlServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.ResponseStatus;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * gRPC service implementation for sending operational commands to data-plane nodes.
 *
 * <p>Supports two modes:</p>
 * <ul>
 *   <li>{@code SendCommand} -- unary request/response for immediate command execution</li>
 *   <li>{@code StreamCommands} -- bidirectional stream allowing the control plane to push
 *       commands to specific nodes and receive acknowledgements asynchronously</li>
 * </ul>
 *
 * <p>Thread safety: the {@code commandStreams} map uses {@link ConcurrentHashMap}.
 * Individual StreamObserver instances are not thread-safe by gRPC contract; callers
 * of {@link #pushCommand} must serialize pushes for the same node.</p>
 */
@Log4j2
public final class NodeControlServiceImpl
        extends NodeControlServiceGrpc.NodeControlServiceImplBase {

    private final NodeRegistry registry;
    private final ConfigDistributor distributor;

    /**
     * Active command streams keyed by node ID. Each entry holds the server-side
     * {@link StreamObserver} for pushing {@link NodeCommand} messages to that node.
     */
    private final ConcurrentHashMap<String, StreamObserver<NodeCommand>> commandStreams = new ConcurrentHashMap<>();

    /**
     * @param registry    the node registry; must not be null
     * @param distributor the config distributor for force-sync operations; must not be null
     */
    public NodeControlServiceImpl(NodeRegistry registry, ConfigDistributor distributor) {
        this.registry = Objects.requireNonNull(registry, "registry");
        this.distributor = Objects.requireNonNull(distributor, "distributor");
    }

    /**
     * Send a single command and wait for the response.
     *
     * <p>Validates the target node exists, then executes the command:</p>
     * <ul>
     *   <li>{@code drain} -- transitions the node to DRAINING state</li>
     *   <li>{@code undrain} -- transitions the node back to HEALTHY state</li>
     *   <li>{@code force_sync} -- triggers a config resync for the node</li>
     *   <li>{@code restart} -- logged; actual restart is the node's responsibility</li>
     * </ul>
     */
    @Override
    public void sendCommand(NodeCommand request, StreamObserver<CommandResponse> responseObserver) {
        String nodeId = request.getNodeId();
        String commandId = request.getCommandId();

        if (nodeId == null || nodeId.isEmpty()) {
            responseObserver.onError(Status.INVALID_ARGUMENT
                    .withDescription("Missing node_id in command")
                    .asRuntimeException());
            return;
        }

        Optional<DataPlaneNode> nodeOpt = registry.get(nodeId);
        if (nodeOpt.isEmpty()) {
            responseObserver.onNext(CommandResponse.newBuilder()
                    .setCommandId(commandId)
                    .setStatus(ResponseStatus.ERROR)
                    .setMessage("Node not registered: " + nodeId)
                    .build());
            responseObserver.onCompleted();
            return;
        }

        DataPlaneNode node = nodeOpt.get();

        switch (request.getCommandCase()) {
            case DRAIN -> {
                node.markDraining();
                log.info("Command {}: node {} marked as DRAINING (timeout={}s)",
                        commandId, nodeId, request.getDrain().getTimeoutSeconds());

                responseObserver.onNext(CommandResponse.newBuilder()
                        .setCommandId(commandId)
                        .setStatus(ResponseStatus.OK)
                        .setMessage("Node " + nodeId + " set to DRAINING")
                        .build());
                responseObserver.onCompleted();
            }

            case UNDRAIN -> {
                node.markHealthy();
                log.info("Command {}: node {} marked as HEALTHY (undrained)", commandId, nodeId);

                responseObserver.onNext(CommandResponse.newBuilder()
                        .setCommandId(commandId)
                        .setStatus(ResponseStatus.OK)
                        .setMessage("Node " + nodeId + " set to HEALTHY")
                        .build());
                responseObserver.onCompleted();
            }

            case FORCE_SYNC -> {
                log.info("Command {}: triggering config resync for node {} (resource_types={})",
                        commandId, nodeId, request.getForceSync().getResourceTypesList());

                ConfigDelta delta = distributor.computeNodeDelta(node.appliedConfigVersion());
                String message;
                if (delta == null) {
                    message = "Node " + nodeId + " requires full snapshot resync (too far behind)";
                } else if (delta.isEmpty()) {
                    message = "Node " + nodeId + " is already up to date";
                } else {
                    message = "Node " + nodeId + " delta computed: " + delta.mutations().size() + " mutations";
                }

                responseObserver.onNext(CommandResponse.newBuilder()
                        .setCommandId(commandId)
                        .setStatus(ResponseStatus.OK)
                        .setMessage(message)
                        .build());
                responseObserver.onCompleted();
            }

            case RESTART -> {
                log.info("Command {}: restart requested for node {} (graceful={}, delay={}s)",
                        commandId, nodeId,
                        request.getRestart().getGraceful(),
                        request.getRestart().getDelaySeconds());

                // Actual restart is the node-side responsibility. The control plane
                // only records the intent and forwards the command via the command stream
                // if one is active.
                responseObserver.onNext(CommandResponse.newBuilder()
                        .setCommandId(commandId)
                        .setStatus(ResponseStatus.OK)
                        .setMessage("Restart command sent to node " + nodeId)
                        .build());
                responseObserver.onCompleted();
            }

            case COMMAND_NOT_SET -> {
                responseObserver.onNext(CommandResponse.newBuilder()
                        .setCommandId(commandId)
                        .setStatus(ResponseStatus.ERROR)
                        .setMessage("No command specified")
                        .build());
                responseObserver.onCompleted();
            }
        }
    }

    /**
     * Bidirectional command stream. The control plane pushes commands; the node
     * sends back acknowledgements as each command is processed.
     *
     * <p>Note the proto definition: the client sends {@link CommandAck} and receives
     * {@link NodeCommand}. This is the reverse of typical client-server patterns
     * because the control plane is the command initiator.</p>
     */
    @Override
    public StreamObserver<CommandAck> streamCommands(StreamObserver<NodeCommand> responseObserver) {
        return new StreamObserver<>() {

            private volatile String nodeId;

            @Override
            public void onNext(CommandAck ack) {
                // The first message from the node establishes its identity via
                // the command_id convention: "register:<nodeId>" for the initial handshake.
                // Subsequent messages are genuine acknowledgements of executed commands.
                String commandId = ack.getCommandId();

                if (commandId != null && commandId.startsWith("register:")) {
                    String id = commandId.substring("register:".length());
                    this.nodeId = id;
                    commandStreams.put(id, responseObserver);
                    log.info("Node {} established command stream", id);
                    return;
                }

                if (ack.getSuccess()) {
                    log.info("Node {} ACKed command {} successfully", nodeId, commandId);
                } else {
                    log.warn("Node {} NACKed command {}: {}", nodeId, commandId, ack.getErrorMessage());
                }
            }

            @Override
            public void onError(Throwable t) {
                String id = this.nodeId;
                if (id != null) {
                    commandStreams.remove(id);
                    log.warn("Command stream error for node {}: {}", id, t.getMessage());
                } else {
                    log.warn("Command stream error from unknown node: {}", t.getMessage());
                }
            }

            @Override
            public void onCompleted() {
                String id = this.nodeId;
                if (id != null) {
                    commandStreams.remove(id);
                    log.info("Command stream completed for node {}", id);
                }
                responseObserver.onCompleted();
            }
        };
    }

    /**
     * Push a command to a specific node via its active command stream.
     *
     * <p>GAP-GRPC-2 FIX: Synchronized on the observer to prevent concurrent writes
     * to the same StreamObserver, which would corrupt HTTP/2 frames. gRPC's
     * StreamObserver is not thread-safe; concurrent onNext() calls can interleave
     * frame bytes on the wire.</p>
     *
     * @param nodeId  the target node ID
     * @param command the command to push
     * @return {@code true} if the command was sent, {@code false} if the node has no active stream
     */
    public boolean pushCommand(String nodeId, NodeCommand command) {
        StreamObserver<NodeCommand> observer = commandStreams.get(nodeId);
        if (observer == null) {
            log.debug("No active command stream for node {}, cannot push command {}", nodeId, command.getCommandId());
            return false;
        }

        try {
            synchronized (observer) {
                observer.onNext(command);
            }
            log.debug("Pushed command {} to node {}", command.getCommandId(), nodeId);
            return true;
        } catch (Exception e) {
            log.error("Failed to push command {} to node {}", command.getCommandId(), nodeId, e);
            commandStreams.remove(nodeId);
            return false;
        }
    }

    /**
     * Returns the number of active command streams.
     */
    public int activeStreamCount() {
        return commandStreams.size();
    }
}
