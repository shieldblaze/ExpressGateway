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
package com.shieldblaze.expressgateway.controlplane;

import com.shieldblaze.expressgateway.controlplane.cluster.ConfigWriteForwarder;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneInstance;
import com.shieldblaze.expressgateway.controlplane.cluster.ReconnectStormProtector;
import com.shieldblaze.expressgateway.controlplane.cluster.RegionAwareNodeAssigner;
import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.distribution.DirectFanOut;
import com.shieldblaze.expressgateway.controlplane.distribution.DistributionMetrics;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistryMetrics;
import com.shieldblaze.expressgateway.controlplane.grpc.server.ConfigDistributionServiceImpl;
import com.shieldblaze.expressgateway.controlplane.grpc.server.ControlPlaneGrpcServer;
import com.shieldblaze.expressgateway.controlplane.grpc.server.NodeControlServiceImpl;
import com.shieldblaze.expressgateway.controlplane.grpc.server.NodeRegistrationServiceImpl;
import com.shieldblaze.expressgateway.controlplane.grpc.server.StatsCollectionServiceImpl;
import com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor.AuthInterceptor;
import com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor.RateLimitInterceptor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreFactory;
import com.shieldblaze.expressgateway.controlplane.registry.HeartbeatTracker;
import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import io.micrometer.core.instrument.MeterRegistry;
import io.netty.handler.ssl.SslContext;
import lombok.extern.log4j.Log4j2;

import javax.net.ssl.SSLException;
import java.io.Closeable;
import java.io.IOException;
import java.util.Collection;
import java.util.Collections;
import java.util.Objects;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Top-level Control Plane server. Manages the lifecycle of all CP components:
 * KV store, node registry, heartbeat tracker, config distributor, and gRPC server.
 *
 * <p>Component wiring and lifecycle:</p>
 * <ol>
 *   <li><b>Construction</b> -- all components are instantiated and wired. The
 *       circular dependency between {@link ConfigDistributor} and
 *       {@link ConfigDistributionServiceImpl} is broken by using the deferred
 *       constructor on the service, then calling {@code setDistributor()} after
 *       the distributor is fully built.</li>
 *   <li><b>{@link #start()}</b> -- starts the heartbeat tracker, config distributor,
 *       and gRPC server, in that order. The heartbeat tracker must be running before
 *       the gRPC server accepts registrations so that newly connected nodes are
 *       immediately monitored.</li>
 *   <li><b>{@link #close()}</b> -- shuts down in reverse order: gRPC server first
 *       (stop accepting new RPCs), then the distributor (drain pending batches),
 *       then the heartbeat tracker. This ensures that no new config pushes are
 *       attempted after the gRPC transport is down.</li>
 * </ol>
 *
 * <p>Thread safety: the {@code started} flag is volatile for safe reads from
 * monitoring threads. The start/close methods themselves are NOT thread-safe --
 * they must be called from a single management thread.</p>
 */
@Log4j2
public final class ControlPlaneServer implements Closeable {

    /**
     * Lifecycle states for the control plane server instance.
     */
    public enum State {
        /** Server has been constructed but not yet started. */
        STARTING,
        /** Server is running and serving traffic. */
        RUNNING,
        /** Server is draining: sending RECONNECT directives and waiting for nodes to disconnect. */
        DRAINING,
        /** Server has been fully stopped. */
        STOPPED
    }

    /** Default timeout for graceful drain in milliseconds (30 seconds). */
    private static final long DEFAULT_DRAIN_TIMEOUT_MS = 30_000;

    private final ControlPlaneConfiguration config;
    private final KVStore kvStore;
    private final NodeRegistry nodeRegistry;
    private final HeartbeatTracker heartbeatTracker;
    private final ChangeJournal journal;
    private final ConfigDistributor distributor;
    private final ConfigDistributionServiceImpl configService;
    private final NodeRegistrationServiceImpl registrationService;
    private final ControlPlaneGrpcServer grpcServer;
    private final String controlPlaneId;
    private final AtomicReference<State> state = new AtomicReference<>(State.STARTING);

    // ---- Cluster components (null when clusterEnabled=false) ----
    private final ControlPlaneCluster cluster;
    private final ReconnectStormProtector reconnectStormProtector;
    private final ConfigWriteForwarder configWriteForwarder;
    private final RegionAwareNodeAssigner regionAwareNodeAssigner;

    /**
     * Creates and starts a {@link ControlPlaneServer}, including KV store initialization
     * and health checks.
     *
     * <p>This is the preferred entry point for production use. It creates the appropriate
     * {@link KVStore} based on the configured {@link ControlPlaneConfiguration.KvStoreType},
     * runs startup health checks, and wires all components.</p>
     *
     * @param config the control plane configuration; must not be null and should
     *               have been {@linkplain ControlPlaneConfiguration#validate() validated}
     * @return a fully wired (but not yet started) ControlPlaneServer
     * @throws KVStoreException if KV store creation, health checks, or journal initialization fails
     */
    public static ControlPlaneServer create(ControlPlaneConfiguration config) throws KVStoreException {
        Objects.requireNonNull(config, "config");
        KVStore kvStore = KVStoreFactory.create(config.kvStoreType(), config.storage());
        return new ControlPlaneServer(config, kvStore);
    }

    /**
     * Constructs and wires all Control Plane components without metrics.
     *
     * <p>The {@link KVStore} must already be connected and ready. The
     * {@link ChangeJournal} constructor reads existing entries from the KV store
     * to recover the current global revision, so the store must be reachable.</p>
     *
     * @param config  the control plane configuration; must not be null and should
     *                have been {@linkplain ControlPlaneConfiguration#validate() validated}
     * @param kvStore the KV store backend; must not be null and must be connected
     * @throws KVStoreException if the ChangeJournal fails to initialize from the KV store
     */
    public ControlPlaneServer(ControlPlaneConfiguration config, KVStore kvStore) throws KVStoreException {
        this(config, kvStore, null);
    }

    /**
     * Constructs and wires all Control Plane components.
     *
     * <p>The {@link KVStore} must already be connected and ready. The
     * {@link ChangeJournal} constructor reads existing entries from the KV store
     * to recover the current global revision, so the store must be reachable.</p>
     *
     * @param config        the control plane configuration; must not be null and should
     *                      have been {@linkplain ControlPlaneConfiguration#validate() validated}
     * @param kvStore       the KV store backend; must not be null and must be connected
     * @param meterRegistry optional Micrometer meter registry for distribution metrics; may be null
     * @throws KVStoreException if the ChangeJournal fails to initialize from the KV store
     */
    public ControlPlaneServer(ControlPlaneConfiguration config, KVStore kvStore, MeterRegistry meterRegistry) throws KVStoreException {
        this.config = Objects.requireNonNull(config, "config");
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        this.controlPlaneId = UUID.randomUUID().toString();

        // ---- Metrics (optional) ----
        DistributionMetrics distMetrics = meterRegistry != null ? new DistributionMetrics(meterRegistry) : null;

        // ---- Core components ----
        this.nodeRegistry = new NodeRegistry();

        this.heartbeatTracker = new HeartbeatTracker(
                nodeRegistry,
                config.heartbeatMissThreshold(),
                config.heartbeatDisconnectThreshold(),
                config.heartbeatScanIntervalMs());

        this.journal = new ChangeJournal(kvStore, "/controlplane/journal");

        // ---- Break circular dependency: ConfigDistributionServiceImpl <-> ConfigDistributor ----
        //
        // The distributor's DirectFanOut callback pushes deltas to nodes via
        // ConfigDistributionServiceImpl.pushToNode(). But the config service needs
        // the distributor to compute deltas on subscribe/fetch. We break the cycle:
        //   1. Create ConfigDistributionServiceImpl with the deferred constructor (no distributor).
        //   2. Create DirectFanOut with a lambda that calls configService.pushToNode().
        //   3. Create ConfigDistributor with the fan-out.
        //   4. Call configService.setDistributor() to complete the wiring.
        this.configService = new ConfigDistributionServiceImpl(nodeRegistry, controlPlaneId, kvStore);

        DirectFanOut fanOut = new DirectFanOut((node, delta) -> {
            try {
                configService.pushToNode(node.nodeId(), delta);
                return true;
            } catch (Exception e) {
                log.error("Failed to push config delta to node {}", node.nodeId(), e);
                return false;
            }
        }, distMetrics);

        this.distributor = new ConfigDistributor(
                journal, nodeRegistry, fanOut,
                config.writeBatchWindowMs(), config.maxJournalLag(), distMetrics);

        // Complete the circular wiring
        configService.setDistributor(distributor);

        // ---- Wire cross-service listeners ----
        // When a heartbeat stream errors, clean up the node's config stream to prevent leaks.
        // This uses a simple reference since registrationService is created below, so we
        // wire this listener after creating it.

        // ---- Cluster components (only when clustering is enabled) ----
        if (config.clusterEnabled()) {
            this.cluster = new ControlPlaneCluster(kvStore, config, controlPlaneId, config.region());
            // Wire the distributor so the leader can process mutations forwarded by followers.
            // This must be done before cluster.start() (called in start()) because the initial
            // leader election may complete synchronously and the watch must be installable.
            cluster.setConfigDistributor(distributor);
            this.reconnectStormProtector = new ReconnectStormProtector(
                    config.reconnectBurst(), config.reconnectRefillRate());
            this.configWriteForwarder = new ConfigWriteForwarder(cluster, kvStore);
            this.regionAwareNodeAssigner = new RegionAwareNodeAssigner(cluster);
            log.info("Cluster mode enabled: region={}, reconnectBurst={}, reconnectRefillRate={}",
                    config.region(), config.reconnectBurst(), config.reconnectRefillRate());
        } else {
            this.cluster = null;
            this.reconnectStormProtector = null;
            this.configWriteForwarder = null;
            this.regionAwareNodeAssigner = null;
        }

        // ---- gRPC service implementations ----
        this.registrationService = new NodeRegistrationServiceImpl(
                nodeRegistry, controlPlaneId, config.heartbeatIntervalMs(), config.maxNodes());

        // Wire heartbeat error -> config stream cleanup
        registrationService.addHeartbeatStreamErrorListener((nodeId, error) -> {
            log.debug("Cleaning up config stream for node {} after heartbeat stream error: {}",
                    nodeId, error.getMessage());
            configService.removeStream(nodeId);
        });

        StatsCollectionServiceImpl statsService = new StatsCollectionServiceImpl(nodeRegistry);
        NodeControlServiceImpl controlService = new NodeControlServiceImpl(nodeRegistry, distributor);

        // ---- Interceptors ----
        AuthInterceptor authInterceptor = new AuthInterceptor(nodeRegistry);
        RateLimitInterceptor rateLimitInterceptor = new RateLimitInterceptor(config.maxRequestsPerSecondPerNode());

        // ---- gRPC TLS ----
        SslContext sslContext = null;
        if (config.grpcTlsEnabled()) {
            try {
                sslContext = ControlPlaneGrpcServer.createSslContext(
                        config.grpcTlsCertPath(),
                        config.grpcTlsKeyPath(),
                        config.grpcTlsCaPath());
                log.info("gRPC TLS context created (cert={}, key={}, ca={})",
                        config.grpcTlsCertPath(), config.grpcTlsKeyPath(),
                        config.grpcTlsCaPath() != null ? config.grpcTlsCaPath() : "none");
            } catch (SSLException e) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to create gRPC TLS context", e);
            }
        }

        // ---- gRPC server ----
        this.grpcServer = new ControlPlaneGrpcServer(
                config, registrationService, configService,
                statsService, controlService,
                authInterceptor, rateLimitInterceptor, sslContext);
    }

    /**
     * Starts all Control Plane components.
     *
     * <p>Start order: heartbeat tracker -> config distributor -> gRPC server.
     * The heartbeat tracker must be running before the gRPC server accepts
     * connections so that newly registered nodes are immediately monitored.</p>
     *
     * @throws Exception        if any component fails to start
     * @throws IllegalStateException if the server has already been started
     */
    public void start() throws Exception {
        if (!state.compareAndSet(State.STARTING, State.RUNNING)) {
            throw new IllegalStateException("Control Plane server is already started (state=" + state.get() + ")");
        }

        log.info("Starting Control Plane server (id={}, clusterEnabled={})", controlPlaneId, config.clusterEnabled());

        boolean clusterStarted = false;
        boolean heartbeatStarted = false;
        boolean distributorStarted = false;
        try {
            // Start cluster first -- it registers in the KV store and begins leader election.
            // This must happen before the gRPC server accepts connections so that leadership
            // state and peer discovery are available when nodes register.
            if (cluster != null) {
                cluster.start();
                clusterStarted = true;
                log.info("Cluster started: isLeader={}, peers={}", cluster.isLeader(), cluster.peers().size());
            }

            heartbeatTracker.start();
            heartbeatStarted = true;

            distributor.start();
            distributorStarted = true;

            grpcServer.start();
        } catch (Exception e) {
            log.error("Control Plane server failed to start, rolling back", e);
            // Roll back in reverse order -- only close components that were started
            if (distributorStarted) {
                try { distributor.close(); } catch (Exception ex) { log.warn("Rollback: failed to close distributor", ex); }
            }
            if (heartbeatStarted) {
                try { heartbeatTracker.close(); } catch (Exception ex) { log.warn("Rollback: failed to close heartbeat tracker", ex); }
            }
            if (clusterStarted) {
                try { cluster.close(); } catch (Exception ex) { log.warn("Rollback: failed to close cluster", ex); }
            }
            state.set(State.STARTING);
            throw e;
        }
        log.info("Control Plane server started successfully on port {}", config.grpcPort());
    }

    /**
     * Initiates a graceful drain of this control plane instance.
     *
     * <p>Drain sequence:</p>
     * <ol>
     *   <li>Marks the instance state as {@link State#DRAINING}.</li>
     *   <li>If clustered, the cluster state is updated implicitly via the
     *       heartbeat mechanism (peers will observe the instance stopping heartbeats).</li>
     *   <li>Sends RECONNECT directives to all connected nodes via their heartbeat
     *       response streams, instructing them to reconnect to another CP instance.</li>
     *   <li>Waits for all nodes to disconnect (with a configurable timeout).</li>
     * </ol>
     *
     * <p>If the drain timeout expires before all nodes disconnect, a warning is logged
     * and the method returns to allow the caller to proceed with a hard shutdown.</p>
     *
     * @param timeoutMs maximum time to wait for nodes to drain in milliseconds; must be > 0
     */
    public void drain(long timeoutMs) {
        if (timeoutMs <= 0) {
            throw new IllegalArgumentException("drain timeoutMs must be > 0, got: " + timeoutMs);
        }

        State current = state.get();
        if (current != State.RUNNING) {
            log.warn("drain() called in state {} (expected RUNNING), skipping", current);
            return;
        }
        if (!state.compareAndSet(State.RUNNING, State.DRAINING)) {
            log.warn("drain() lost CAS race (concurrent drain/close), skipping");
            return;
        }

        log.info("Initiating graceful drain (timeout={}ms, connectedNodes={})",
                timeoutMs, nodeRegistry.size());

        // Send RECONNECT directives to all connected nodes.
        // The directive tells them to reconnect to another CP instance (empty target_address
        // means the node should use its service discovery / DNS to find a new CP instance).
        sendReconnectDirectivesToAllNodes();

        // Wait for nodes to disconnect, polling at 500ms intervals.
        long deadline = System.currentTimeMillis() + timeoutMs;
        while (System.currentTimeMillis() < deadline && nodeRegistry.size() > 0) {
            try {
                Thread.sleep(Math.max(0, Math.min(500, deadline - System.currentTimeMillis())));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                log.warn("Drain wait interrupted with {} nodes still connected", nodeRegistry.size());
                return;
            }
        }

        int remaining = nodeRegistry.size();
        if (remaining > 0) {
            log.warn("Drain timeout expired with {} nodes still connected, proceeding to shutdown", remaining);
        } else {
            log.info("All nodes disconnected during drain");
        }
    }

    /**
     * Convenience drain with the default 30-second timeout.
     */
    public void drain() {
        drain(DEFAULT_DRAIN_TIMEOUT_MS);
    }

    /**
     * Sends a RECONNECT directive to all connected nodes via their config streams.
     * Nodes that fail to receive the directive are logged and skipped.
     */
    private void sendReconnectDirectivesToAllNodes() {
        for (var node : nodeRegistry.allNodes()) {
            try {
                // Remove the node's config stream -- this triggers onCompleted() which
                // signals the node to reconnect. The removeStream method handles the
                // safe completion of the observer.
                configService.removeStream(node.nodeId());
            } catch (Exception e) {
                log.warn("Failed to send reconnect signal to node {}", node.nodeId(), e);
            }
        }
    }

    /**
     * Shuts down all Control Plane components in reverse start order.
     *
     * <p>If the server is currently in {@link State#RUNNING}, {@link #drain()} is called
     * first to gracefully disconnect nodes before tearing down components.</p>
     *
     * <p>Shutdown order: gRPC server -> config distributor -> heartbeat tracker -> cluster.</p>
     */
    @Override
    public void close() throws IOException {
        State current = state.get();
        if (current == State.STOPPED) {
            return; // Already closed
        }
        if (current == State.STARTING) {
            // Never started, just mark stopped
            state.set(State.STOPPED);
            return;
        }

        // If RUNNING, initiate drain first
        if (current == State.RUNNING) {
            drain();
        }

        // Now transition to STOPPED (from DRAINING or RUNNING if drain() was skipped)
        state.set(State.STOPPED);

        log.info("Shutting down Control Plane server (id={})", controlPlaneId);

        grpcServer.close();
        distributor.close();
        heartbeatTracker.close();

        // Shut down cluster components after gRPC/distributor/heartbeat are stopped
        // but before the KV store is closed (if the caller manages KV store lifecycle).
        // ConfigWriteForwarder is closed first because it uses the cluster for leadership checks.
        if (configWriteForwarder != null) {
            try { configWriteForwarder.close(); } catch (Exception e) { log.warn("Failed to close ConfigWriteForwarder", e); }
        }
        if (cluster != null) {
            try { cluster.close(); } catch (Exception e) { log.warn("Failed to close ControlPlaneCluster", e); }
        }

        // Close KV store last -- all other components depend on it
        try { kvStore.close(); } catch (Exception e) { log.warn("Failed to close KVStore", e); }

        log.info("Control Plane server shut down");
    }

    /**
     * Returns the node registry for external management queries.
     */
    public NodeRegistry nodeRegistry() {
        return nodeRegistry;
    }

    /**
     * Returns the config distributor for submitting config mutations.
     */
    public ConfigDistributor configDistributor() {
        return distributor;
    }

    /**
     * Returns the change journal for config versioning and delta sync.
     */
    public ChangeJournal changeJournal() {
        return journal;
    }

    /**
     * Returns the actual port the gRPC server is listening on.
     * Useful when the configured port is 0 (ephemeral port for testing).
     *
     * @return the listening port, or -1 if the server has not started
     */
    public int grpcPort() {
        return grpcServer.getPort();
    }

    /**
     * Returns the unique identifier of this control plane instance.
     * Used for correlating logs and config push nonces across a multi-instance deployment.
     */
    public String controlPlaneId() {
        return controlPlaneId;
    }

    /**
     * Returns whether the server has been started and not yet shut down.
     * A server in DRAINING state is still considered "started".
     */
    public boolean isStarted() {
        State s = state.get();
        return s == State.RUNNING || s == State.DRAINING;
    }

    /**
     * Returns the current lifecycle state of this server.
     *
     * @return the current {@link State}
     */
    public State serverState() {
        return state.get();
    }

    /**
     * Initializes Micrometer metrics for config distribution and node registry.
     *
     * <p>Call this after construction to wire metrics. If {@code meterRegistry} is
     * {@code null}, this method is a no-op, preserving backward compatibility for
     * environments without a metrics backend.</p>
     *
     * @param meterRegistry the Micrometer meter registry, or {@code null} to skip metrics
     */
    public void initMetrics(MeterRegistry meterRegistry) {
        if (meterRegistry != null) {
            // DistributionMetrics are wired through the 3-arg constructor into
            // DirectFanOut and ConfigDistributor. Use new ControlPlaneServer(config,
            // kvStore, meterRegistry) to enable push success/failure/latency tracking.
            new NodeRegistryMetrics(nodeRegistry, meterRegistry); // registers itself
        }
    }

    // ---- Cluster accessors ----

    /**
     * Returns whether clustering is enabled for this server instance.
     */
    public boolean isClusterEnabled() {
        return config.clusterEnabled();
    }

    /**
     * Returns whether this instance is the cluster leader.
     * Always returns {@code true} in single-node mode (clusterEnabled=false).
     */
    public boolean isLeader() {
        return cluster == null || cluster.isLeader();
    }

    /**
     * Returns the known cluster peers (including this instance).
     * Returns an empty collection in single-node mode.
     *
     * @return unmodifiable collection of peer instances, or empty if clustering is disabled
     */
    public Collection<ControlPlaneInstance> clusterPeers() {
        return cluster != null ? cluster.peers() : Collections.emptyList();
    }

    /**
     * Returns the {@link ControlPlaneCluster}, or {@code null} if clustering is disabled.
     */
    public ControlPlaneCluster cluster() {
        return cluster;
    }

    /**
     * Returns the {@link ReconnectStormProtector}, or {@code null} if clustering is disabled.
     */
    public ReconnectStormProtector reconnectStormProtector() {
        return reconnectStormProtector;
    }

    /**
     * Returns the {@link ConfigWriteForwarder} for forwarding config mutations
     * from follower instances to the leader, or {@code null} if clustering is disabled.
     */
    public ConfigWriteForwarder configWriteForwarder() {
        return configWriteForwarder;
    }

    /**
     * Returns the {@link RegionAwareNodeAssigner}, or {@code null} if clustering is disabled.
     */
    public RegionAwareNodeAssigner regionAwareNodeAssigner() {
        return regionAwareNodeAssigner;
    }
}
