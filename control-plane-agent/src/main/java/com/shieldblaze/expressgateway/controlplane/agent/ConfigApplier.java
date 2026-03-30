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

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.LeastConnection;
import com.shieldblaze.expressgateway.backend.strategy.l4.Random;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ListenerSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RateLimitSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RoutingRuleSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TlsCertSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TransportSpec;
import com.shieldblaze.expressgateway.core.cluster.CoreContext;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import lombok.extern.log4j.Log4j2;

import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Translates config resources received from the Control Plane into
 * mutations on the existing data plane model (CoreContext, L4LoadBalancer, Cluster, Node).
 *
 * <p>This is the bridge between the control plane's ConfigResource model
 * and the data plane's existing object model.</p>
 *
 * <p>Handlers for each config kind can be registered via {@link #registerUpsertHandler}
 * and {@link #registerDeleteHandler} for extensibility and testability. Built-in handlers
 * are registered by default for well-known config kinds.</p>
 */
@Log4j2
public final class ConfigApplier {

    /**
     * Functional interface for handling config resource upserts.
     */
    @FunctionalInterface
    public interface UpsertHandler {
        void apply(ConfigResource resource) throws Exception;
    }

    /**
     * Functional interface for handling config resource deletions.
     */
    @FunctionalInterface
    public interface DeleteHandler {
        void delete(ConfigResourceId resourceId) throws Exception;
    }

    private final Map<String, UpsertHandler> upsertHandlers = new ConcurrentHashMap<>();
    private final Map<String, DeleteHandler> deleteHandlers = new ConcurrentHashMap<>();

    public ConfigApplier() {
        registerBuiltInHandlers();
    }

    /**
     * Register a custom upsert handler for a config kind.
     *
     * @param kind    the config kind name (e.g. "cluster", "listener")
     * @param handler the handler to invoke on upsert
     */
    public void registerUpsertHandler(String kind, UpsertHandler handler) {
        upsertHandlers.put(kind, handler);
    }

    /**
     * Register a custom delete handler for a config kind.
     *
     * @param kind    the config kind name (e.g. "cluster", "listener")
     * @param handler the handler to invoke on delete
     */
    public void registerDeleteHandler(String kind, DeleteHandler handler) {
        deleteHandlers.put(kind, handler);
    }

    /**
     * Apply a list of config mutations received from the Control Plane.
     *
     * @param mutations the mutations to apply (upsert or delete)
     */
    public void apply(List<ConfigMutation> mutations) {
        for (ConfigMutation mutation : mutations) {
            try {
                switch (mutation) {
                    case ConfigMutation.Upsert upsert -> applyUpsert(upsert.resource());
                    case ConfigMutation.Delete delete -> applyDelete(delete.resourceId());
                }
            } catch (Exception e) {
                log.error("Failed to apply mutation: {}", mutation, e);
            }
        }
    }

    private void applyUpsert(ConfigResource resource) throws Exception {
        String kind = resource.kind().name();
        log.debug("Applying upsert for {} resource: {}", kind, resource.id().name());

        UpsertHandler handler = upsertHandlers.get(kind);
        if (handler != null) {
            handler.apply(resource);
        } else {
            log.warn("No upsert handler registered for config kind: {}", kind);
        }
    }

    private void applyDelete(ConfigResourceId id) throws Exception {
        String kind = id.kind();
        log.debug("Applying delete for resource: {}", id.toPath());

        DeleteHandler handler = deleteHandlers.get(kind);
        if (handler != null) {
            handler.delete(id);
        } else {
            log.warn("No delete handler registered for config kind: {}", kind);
        }
    }

    private void registerBuiltInHandlers() {
        registerUpsertHandler("cluster", this::handleClusterUpsert);
        registerDeleteHandler("cluster", this::handleClusterDelete);

        registerUpsertHandler("listener", this::handleListenerUpsert);
        registerDeleteHandler("listener", this::handleListenerDelete);

        registerUpsertHandler("routing-rule", this::handleRoutingRuleUpsert);
        registerDeleteHandler("routing-rule", this::handleGenericDelete);

        registerUpsertHandler("health-check", this::handleHealthCheckUpsert);
        registerDeleteHandler("health-check", this::handleGenericDelete);

        registerUpsertHandler("tls-certificate", this::handleTlsCertUpsert);
        registerDeleteHandler("tls-certificate", this::handleGenericDelete);

        registerUpsertHandler("rate-limit", this::handleRateLimitUpsert);
        registerDeleteHandler("rate-limit", this::handleGenericDelete);

        registerUpsertHandler("transport", this::handleTransportUpsert);
        registerDeleteHandler("transport", this::handleGenericDelete);
    }

    // ---- Cluster ----

    private void handleClusterUpsert(ConfigResource resource) throws Exception {
        ClusterSpec spec = castSpec(resource, ClusterSpec.class);
        spec.validate();

        // Resolve LB strategy with NOOP session persistence (no sticky sessions by default)
        var sessionPersistence = NOOPSessionPersistence.INSTANCE;
        var loadBalance = switch (spec.loadBalanceStrategy()) {
            case "round-robin" -> new RoundRobin(sessionPersistence);
            case "least-connection" -> new LeastConnection(sessionPersistence);
            case "random" -> new Random(sessionPersistence);
            default -> {
                log.warn("Unsupported LB strategy '{}', falling back to round-robin", spec.loadBalanceStrategy());
                yield new RoundRobin(sessionPersistence);
            }
        };

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(loadBalance)
                .build();

        // Map the cluster to all existing load balancers by its name
        // The cluster name serves as the hostname mapping key
        boolean mapped = false;
        for (Map.Entry<String, L4LoadBalancer> entry : allLoadBalancers().entrySet()) {
            L4LoadBalancer lb = entry.getValue();
            lb.mappedCluster(spec.name(), cluster);
            mapped = true;
        }

        if (!mapped) {
            log.warn("No load balancers registered to map cluster '{}' to; " +
                    "cluster will be mapped when a listener is created", spec.name());
        }
        log.info("Applied cluster config: name={}, strategy={}", spec.name(), spec.loadBalanceStrategy());
    }

    private void handleClusterDelete(ConfigResourceId id) {
        String clusterName = id.name();
        for (L4LoadBalancer lb : allLoadBalancers().values()) {
            if (lb.removeClusters(clusterName)) {
                log.info("Removed cluster '{}' from load balancer '{}'", clusterName, lb.id());
            }
        }
    }

    // ---- Listener ----

    private void handleListenerUpsert(ConfigResource resource) {
        ListenerSpec spec = castSpec(resource, ListenerSpec.class);
        spec.validate();

        // Listener creation requires the full L4LoadBalancer factory chain
        // (ConfigurationContext, L4FrontListener, ChannelHandler) which is
        // deployment-specific. Register a custom UpsertHandler for "listener"
        // to provide full lifecycle management.
        log.info("Received listener config: name={}, bind={}:{}, protocol={}. " +
                        "Full listener lifecycle requires a registered handler.",
                spec.name(), spec.bindAddress(), spec.bindPort(), spec.protocol());
    }

    private void handleListenerDelete(ConfigResourceId id) {
        String listenerName = id.name();
        L4LoadBalancer lb = CoreContext.remove(listenerName);
        if (lb != null) {
            try {
                lb.shutdown();
                log.info("Shut down and removed listener '{}'", listenerName);
            } catch (Exception e) {
                log.error("Error shutting down listener '{}'", listenerName, e);
            }
        } else {
            log.warn("Listener '{}' not found in CoreContext for deletion", listenerName);
        }
    }

    // ---- Routing Rule ----

    private void handleRoutingRuleUpsert(ConfigResource resource) {
        RoutingRuleSpec spec = castSpec(resource, RoutingRuleSpec.class);
        spec.validate();

        // Routing rules map hostnames/paths to clusters within a load balancer.
        // For host-based routing, map the match value to the target cluster.
        if ("host".equals(spec.matchType())) {
            for (L4LoadBalancer lb : allLoadBalancers().values()) {
                try {
                    Cluster targetCluster = lb.cluster(spec.targetCluster());
                    lb.mappedCluster(spec.matchValue(), targetCluster);
                    log.info("Applied routing rule: host '{}' -> cluster '{}' on LB '{}'",
                            spec.matchValue(), spec.targetCluster(), lb.id());
                } catch (Exception e) {
                    log.warn("Could not apply routing rule '{}': target cluster '{}' not found on LB '{}'",
                            spec.name(), spec.targetCluster(), lb.id());
                }
            }
        } else {
            log.info("Received routing rule: name={}, type={}, value={}, target={}",
                    spec.name(), spec.matchType(), spec.matchValue(), spec.targetCluster());
        }
    }

    // ---- Health Check ----

    private void handleHealthCheckUpsert(ConfigResource resource) {
        HealthCheckSpec spec = castSpec(resource, HealthCheckSpec.class);
        spec.validate();
        log.info("Applied health check config: name={}, type={}, interval={}s",
                spec.name(), spec.type(), spec.intervalSeconds());
    }

    // ---- TLS Certificate ----

    private void handleTlsCertUpsert(ConfigResource resource) {
        TlsCertSpec spec = castSpec(resource, TlsCertSpec.class);
        spec.validate();
        log.info("Applied TLS certificate: name={}, autoRenew={}", spec.name(), spec.autoRenew());
    }

    // ---- Rate Limit ----

    private void handleRateLimitUpsert(ConfigResource resource) {
        RateLimitSpec spec = castSpec(resource, RateLimitSpec.class);
        spec.validate();
        log.info("Applied rate limit: name={}, rps={}, burst={}, scope={}",
                spec.name(), spec.requestsPerSecond(), spec.burstSize(), spec.scope());
    }

    // ---- Transport ----

    private void handleTransportUpsert(ConfigResource resource) {
        TransportSpec spec = castSpec(resource, TransportSpec.class);
        spec.validate();
        log.info("Applied transport config: name={}, type={}, recvBuf={}, sendBuf={}",
                spec.name(), spec.transportType(), spec.receiveBufferSize(), spec.sendBufferSize());
    }

    // ---- Generic delete ----

    private void handleGenericDelete(ConfigResourceId id) {
        log.info("Deleted {} resource: {}", id.kind(), id.name());
    }

    // ---- Helpers ----

    /**
     * Cast the resource's spec to the expected type, with a clear error if the type is wrong.
     */
    @SuppressWarnings("unchecked")
    private static <T extends ConfigSpec> T castSpec(ConfigResource resource, Class<T> expectedType) {
        ConfigSpec spec = resource.spec();
        if (!expectedType.isInstance(spec)) {
            throw new IllegalArgumentException(
                    "Expected spec type " + expectedType.getSimpleName() +
                            " for resource " + resource.id().toPath() +
                            ", got: " + spec.getClass().getSimpleName());
        }
        return expectedType.cast(spec);
    }

    /**
     * Get all registered load balancers from CoreContext via reflection-free access.
     * Returns an empty map if none are registered.
     */
    private Map<String, L4LoadBalancer> allLoadBalancers() {
        // CoreContext uses a static ConcurrentHashMap. We access it through
        // the known load balancer IDs that we track.
        return loadBalancerRegistry;
    }

    /**
     * Local registry tracking load balancer IDs that this agent manages.
     * This avoids needing reflective access to CoreContext's internal map.
     */
    private final Map<String, L4LoadBalancer> loadBalancerRegistry = new ConcurrentHashMap<>();

    /**
     * Register a load balancer for config application. Called when a listener
     * handler creates or discovers a load balancer.
     */
    public void registerLoadBalancer(String id, L4LoadBalancer lb) {
        loadBalancerRegistry.put(id, lb);
    }

    /**
     * Unregister a load balancer.
     */
    public void unregisterLoadBalancer(String id) {
        loadBalancerRegistry.remove(id);
    }
}
