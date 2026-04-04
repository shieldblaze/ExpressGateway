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

import com.shieldblaze.expressgateway.controlplane.v1.ConfigDistributionServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.DeregisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.DeregistrationReason;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import com.shieldblaze.expressgateway.controlplane.v1.NodeRegistrationServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterRequest;
import com.shieldblaze.expressgateway.controlplane.v1.RegisterResponse;
import io.grpc.ManagedChannel;
import io.grpc.netty.GrpcSslContexts;
import io.grpc.netty.NettyChannelBuilder;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import lombok.extern.slf4j.Slf4j;

import java.io.Closeable;
import java.net.InetAddress;
import java.util.List;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Data plane agent that connects to the Control Plane, receives config,
 * and applies it to the local load balancer.
 *
 * <p>Lifecycle: connect -> register -> subscribe to config -> heartbeat -> apply config.
 * On disconnect: operate on last-known-good config, reconnect with backoff.</p>
 */
@Slf4j
public final class ControlPlaneAgent implements Closeable {

    private final AgentConfiguration config;
    private final ConfigApplier applier;
    private final LKGStore lkgStore;
    private final ReconnectStrategy reconnectStrategy;
    private final ScheduledExecutorService reconnectScheduler;

    private volatile ManagedChannel channel;
    private volatile String sessionToken;
    private volatile HeartbeatSender heartbeatSender;
    private volatile ConfigStreamHandler configStreamHandler;
    private volatile ScheduledFuture<?> pendingReconnect;
    private volatile boolean running;

    /**
     * Create a new agent with the given configuration.
     */
    public ControlPlaneAgent(AgentConfiguration config) {
        this.config = config;
        this.applier = new ConfigApplier();
        this.lkgStore = new LKGStore(config.lkgPath());
        this.reconnectScheduler = Executors.newSingleThreadScheduledExecutor(Thread.ofVirtual()
                .name("cp-agent-reconnect")
                .factory());
        this.reconnectStrategy = new ReconnectStrategy();
    }

    /**
     * Backwards-compatible constructor for callers not yet using AgentConfiguration.
     */
    public ControlPlaneAgent(String controlPlaneAddress, int controlPlanePort, String nodeId,
                             String clusterId, String environment, java.nio.file.Path lkgPath) {
        this(AgentConfiguration.plaintext(controlPlaneAddress, controlPlanePort,
                nodeId, clusterId, environment, lkgPath));
    }

    /**
     * Get the config applier for registering custom handlers.
     */
    public ConfigApplier applier() {
        return applier;
    }

    /**
     * Start the agent. Connects to CP and begins config streaming.
     */
    public void start() {
        running = true;
        connect();
    }

    private void connect() {
        try {
            log.info("Connecting to Control Plane at {}:{}",
                    config.controlPlaneAddress(), config.controlPlanePort());

            // Build gRPC channel with optional mTLS
            NettyChannelBuilder channelBuilder = NettyChannelBuilder
                    .forAddress(config.controlPlaneAddress(), config.controlPlanePort())
                    .keepAliveTime(30, TimeUnit.SECONDS)
                    .keepAliveTimeout(10, TimeUnit.SECONDS);

            if (config.tlsEnabled()) {
                SslContext sslContext = GrpcSslContexts.configure(
                                SslContextBuilder.forClient()
                                        .keyManager(config.tlsCertPath().toFile(), config.tlsKeyPath().toFile())
                                        .trustManager(config.tlsTrustCertPath().toFile()))
                        .build();
                channelBuilder.sslContext(sslContext);
            } else {
                channelBuilder.usePlaintext();
            }

            channel = channelBuilder.build();

            // Resolve local address for registration
            String localAddress = config.localAddress();
            if (localAddress.isEmpty()) {
                localAddress = InetAddress.getLocalHost().getHostAddress();
            }

            // Register with CP
            var blockingStub = NodeRegistrationServiceGrpc.newBlockingStub(channel);
            RegisterResponse response = blockingStub.register(RegisterRequest.newBuilder()
                    .setIdentity(NodeIdentity.newBuilder()
                            .setNodeId(config.nodeId())
                            .setClusterId(config.clusterId())
                            .setEnvironment(config.environment())
                            .setAddress(localAddress)
                            .setBuildVersion(config.buildVersion())
                            .build())
                    .addAllSubscribedResourceTypes(List.of("cluster", "listener", "routing-rule",
                            "health-check", "tls-certificate", "rate-limit", "transport"))
                    .setAuthToken(config.authToken())
                    .build());

            if (!response.getAccepted()) {
                log.error("Registration rejected: {}", response.getRejectReason());
                scheduleReconnect();
                return;
            }

            sessionToken = response.getSessionToken();
            long heartbeatIntervalMs = response.getHeartbeatIntervalMs();
            log.info("Registered with CP (id={}), heartbeat interval={}ms",
                    response.getControlPlaneId(), heartbeatIntervalMs);

            reconnectStrategy.reset();

            // Start heartbeat sender with directive callbacks
            var asyncStub = NodeRegistrationServiceGrpc.newStub(channel);
            heartbeatSender = new HeartbeatSender(config.nodeId(), sessionToken, heartbeatIntervalMs);
            heartbeatSender.setReconnectCallback((targetAddress, reason) -> {
                log.info("Reconnect directive received: target={}, reason={}", targetAddress, reason);
                scheduleReconnect();
            });
            heartbeatSender.setResubscribeCallback(resourceTypes -> {
                log.info("Resubscribe directive received for types: {}", resourceTypes);
                // Close and restart the config stream to resubscribe
                ConfigStreamHandler handler = configStreamHandler;
                if (handler != null) {
                    handler.close();
                    var configStub = ConfigDistributionServiceGrpc.newStub(channel);
                    handler = new ConfigStreamHandler(config.nodeId(), sessionToken, applier, lkgStore);
                    handler.start(configStub);
                    configStreamHandler = handler;
                }
            });
            heartbeatSender.start(asyncStub);

            // Start config stream
            var configStub = ConfigDistributionServiceGrpc.newStub(channel);
            configStreamHandler = new ConfigStreamHandler(config.nodeId(), sessionToken, applier, lkgStore);
            configStreamHandler.start(configStub);

            log.info("Control Plane agent fully connected and streaming");

        } catch (Exception e) {
            log.error("Failed to connect to Control Plane", e);
            scheduleReconnect();
        }
    }

    private void scheduleReconnect() {
        if (!running) {
            return;
        }
        long delay = reconnectStrategy.nextDelay();
        log.info("Scheduling reconnect in {}ms", delay);
        pendingReconnect = reconnectScheduler.schedule(() -> {
            if (running) {
                cleanup();
                connect();
            }
        }, delay, TimeUnit.MILLISECONDS);
    }

    private synchronized void cleanup() {
        if (heartbeatSender != null) {
            heartbeatSender.close();
            heartbeatSender = null;
        }
        if (configStreamHandler != null) {
            configStreamHandler.close();
            configStreamHandler = null;
        }
        if (channel != null && !channel.isShutdown()) {
            channel.shutdown();
            try {
                if (!channel.awaitTermination(5, TimeUnit.SECONDS)) {
                    channel.shutdownNow();
                }
            } catch (InterruptedException e) {
                channel.shutdownNow();
                Thread.currentThread().interrupt();
            }
        }
    }

    @Override
    public void close() {
        log.info("Shutting down Control Plane agent");
        running = false;

        // Cancel any pending reconnect
        ScheduledFuture<?> pending = pendingReconnect;
        if (pending != null) {
            pending.cancel(false);
        }

        // Shut down the reconnect scheduler
        reconnectScheduler.shutdown();
        try {
            if (!reconnectScheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                reconnectScheduler.shutdownNow();
            }
        } catch (InterruptedException e) {
            reconnectScheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }

        // Deregister from CP if connected
        ManagedChannel ch = channel;
        String token = sessionToken;
        if (ch != null && !ch.isShutdown() && token != null) {
            try {
                var blockingStub = NodeRegistrationServiceGrpc.newBlockingStub(ch);
                blockingStub.deregister(DeregisterRequest.newBuilder()
                        .setNodeId(config.nodeId())
                        .setSessionToken(token)
                        .setReason(DeregistrationReason.SHUTDOWN)
                        .build());
            } catch (Exception e) {
                log.warn("Failed to deregister from CP during shutdown", e);
            }
        }

        cleanup();
        log.info("Control Plane agent shut down");
    }
}
