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

import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ListenerSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RateLimitSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RoutingRuleSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.SecurityPolicySpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TlsCertSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TransportSpec;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigAck;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigDistributionServiceGrpc;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigError;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigNack;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigRequest;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigResponse;
import com.shieldblaze.expressgateway.controlplane.v1.ConfigSubscription;
import com.shieldblaze.expressgateway.controlplane.v1.Resource;
import io.grpc.stub.StreamObserver;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.time.Instant;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * Manages the bidirectional config distribution stream.
 * Subscribes to config types, receives updates, sends ACK/NACK.
 *
 * <p>Follows the ADS (Aggregated Discovery Service) pattern: the agent sends
 * subscription requests and ACK/NACK responses; the control plane pushes
 * configuration updates as they become available.</p>
 *
 * <p>Thread safety: all writes to the gRPC {@code requestObserver} are serialized
 * through {@code writeLock}. This is required because gRPC StreamObservers are
 * NOT thread-safe: {@code subscribe()} is called from the start thread while
 * ACK/NACK responses are sent from the gRPC callback thread.</p>
 */
@Log4j2
public final class ConfigStreamHandler implements Closeable {

    private static final ObjectMapper MAPPER = new ObjectMapper().findAndRegisterModules();

    /**
     * Maps config kind names (type_url values) to their concrete ConfigSpec classes.
     * The server serializes specs as plain JSON (without Jackson @class discriminator),
     * so the agent must use the proto type_url field to determine which class to deserialize to.
     */
    private static final Map<String, Class<? extends ConfigSpec>> SPEC_TYPE_REGISTRY = Map.of(
            "cluster", ClusterSpec.class,
            "listener", ListenerSpec.class,
            "routing-rule", RoutingRuleSpec.class,
            "health-check", HealthCheckSpec.class,
            "tls-certificate", TlsCertSpec.class,
            "rate-limit", RateLimitSpec.class,
            "security-policy", SecurityPolicySpec.class,
            "transport", TransportSpec.class
    );

    private final String nodeId;
    private final String sessionToken;
    private final ConfigApplier applier;
    private final LKGStore lkgStore;
    private volatile StreamObserver<ConfigRequest> requestObserver;
    private volatile long lastAppliedVersion;
    private volatile boolean running;

    /**
     * Lock that serializes all writes ({@code onNext}, {@code onCompleted}) to
     * the gRPC {@code requestObserver}. gRPC's StreamObserver is NOT thread-safe;
     * concurrent calls from the start thread (subscribe) and the gRPC callback
     * thread (ACK/NACK in handleConfigResponse) would corrupt HTTP/2 frames.
     */
    private final Object writeLock = new Object();

    // Well-known resource types to subscribe to
    private static final List<String> DEFAULT_RESOURCE_TYPES = List.of(
            "cluster", "listener", "routing-rule", "health-check",
            "tls-certificate", "rate-limit", "transport"
    );

    public ConfigStreamHandler(String nodeId, String sessionToken,
                               ConfigApplier applier, LKGStore lkgStore) {
        this.nodeId = nodeId;
        this.sessionToken = sessionToken;
        this.applier = applier;
        this.lkgStore = lkgStore;
        this.lastAppliedVersion = lkgStore.loadRevision();
    }

    /**
     * Start the config stream using the provided gRPC stub.
     *
     * @param stub the async stub for the ConfigDistributionService
     */
    public void start(ConfigDistributionServiceGrpc.ConfigDistributionServiceStub stub) {
        running = true;

        requestObserver = stub.streamConfig(new StreamObserver<>() {
            @Override
            public void onNext(ConfigResponse response) {
                handleConfigResponse(response);
            }

            @Override
            public void onError(Throwable t) {
                log.warn("Config stream error", t);
                running = false;
            }

            @Override
            public void onCompleted() {
                log.info("Config stream completed");
                running = false;
            }
        });

        // Subscribe to all default resource types
        for (String type : DEFAULT_RESOURCE_TYPES) {
            subscribe(type);
        }
    }

    private void subscribe(String typeUrl) {
        StreamObserver<ConfigRequest> observer = requestObserver;
        if (observer == null) {
            return;
        }
        synchronized (writeLock) {
            observer.onNext(ConfigRequest.newBuilder()
                    .setNodeId(nodeId)
                    .setSessionToken(sessionToken)
                    .setSubscribe(ConfigSubscription.newBuilder()
                            .setTypeUrl(typeUrl)
                            .setVersion(lastAppliedVersion > 0 ? String.valueOf(lastAppliedVersion) : "")
                            .build())
                    .build());
        }
    }

    private void handleConfigResponse(ConfigResponse response) {
        try {
            log.info("Received config update: type={}, version={}, resources={}, removed={}, snapshot={}",
                    response.getTypeUrl(), response.getVersion(),
                    response.getResourcesCount(), response.getRemovedResourcesCount(),
                    response.getIsFullSnapshot());

            // Deserialize proto Resources into ConfigMutations
            List<ConfigMutation> mutations = deserializeMutations(response);

            // Apply the config before ACKing -- the control plane must not be told
            // a config version was applied if it actually was not.
            applier.apply(mutations);

            // Send ACK, then update version atomically after the write succeeds.
            // This ensures lastAppliedVersion is only advanced if the ACK was
            // actually sent on the wire.
            long version = Long.parseLong(response.getVersion());
            synchronized (writeLock) {
                requestObserver.onNext(ConfigRequest.newBuilder()
                        .setNodeId(nodeId)
                        .setSessionToken(sessionToken)
                        .setAck(ConfigAck.newBuilder()
                                .setTypeUrl(response.getTypeUrl())
                                .setVersion(response.getVersion())
                                .setResponseNonce(response.getNonce())
                                .build())
                        .build());
            }
            lastAppliedVersion = version;

            // Persist the applied config as last-known-good so the data plane can
            // boot from it when disconnected from the control plane.
            List<ConfigResource> upsertedResources = mutations.stream()
                    .filter(m -> m instanceof ConfigMutation.Upsert)
                    .map(m -> ((ConfigMutation.Upsert) m).resource())
                    .toList();
            try {
                lkgStore.save(upsertedResources, version);
            } catch (Exception lkgErr) {
                log.warn("Failed to persist LKG config at version {}", version, lkgErr);
            }

            log.info("ACKed config version {}", response.getVersion());

        } catch (Exception e) {
            log.error("Failed to apply config update", e);
            // NACK the config
            StreamObserver<ConfigRequest> observer = requestObserver;
            if (observer != null) {
                synchronized (writeLock) {
                    observer.onNext(ConfigRequest.newBuilder()
                            .setNodeId(nodeId)
                            .setSessionToken(sessionToken)
                            .setNack(ConfigNack.newBuilder()
                                    .setTypeUrl(response.getTypeUrl())
                                    .setVersion(response.getVersion())
                                    .setResponseNonce(response.getNonce())
                                    .setError(ConfigError.newBuilder()
                                            .setCode(1)
                                            .setMessage(e.getMessage() != null ? e.getMessage() : "unknown error")
                                            .build())
                                    .build())
                            .build());
                }
            }
        }
    }

    /**
     * Deserialize a {@link ConfigResponse} into a list of {@link ConfigMutation} objects.
     *
     * <p>The server serializes each resource's {@link ConfigSpec} as plain JSON bytes
     * in the proto {@code Resource.payload} field (via ConfigDistributionServiceImpl.toProtoResource).
     * The agent uses the proto {@code type_url} field to determine which concrete
     * ConfigSpec class to deserialize into, since the JSON payload does not carry
     * a Jackson type discriminator.</p>
     *
     * <p>Delete mutations are carried as full resource paths ({@code kind/scopeQualifier/name})
     * in {@code removed_resources}.</p>
     */
    List<ConfigMutation> deserializeMutations(ConfigResponse response) throws Exception {
        List<ConfigMutation> mutations = new ArrayList<>();
        String typeUrl = response.getTypeUrl();
        long version = Long.parseLong(response.getVersion());
        Instant now = Instant.now();

        // Upserts: each Resource carries the spec as JSON bytes in its payload
        for (Resource resource : response.getResourcesList()) {
            String kindName = resource.getTypeUrl().isEmpty() ? typeUrl : resource.getTypeUrl();

            // Resolve the concrete ConfigSpec class from the type_url
            Class<? extends ConfigSpec> specClass = SPEC_TYPE_REGISTRY.get(kindName);
            if (specClass == null) {
                log.warn("Unknown config kind '{}', skipping resource '{}'", kindName, resource.getName());
                continue;
            }

            ConfigSpec spec = MAPPER.readValue(resource.getPayload().toByteArray(), specClass);
            ConfigKind kind = new ConfigKind(kindName, 1);
            ConfigScope scope = new ConfigScope.Global();
            ConfigResourceId id = new ConfigResourceId(kindName, scope.qualifier(), resource.getName());
            long resourceVersion = resource.getVersion().isEmpty() ? version : Long.parseLong(resource.getVersion());

            ConfigResource configResource = new ConfigResource(
                    id, kind, scope, resourceVersion, now, now,
                    "control-plane", Map.of(), spec
            );
            mutations.add(new ConfigMutation.Upsert(configResource));
        }

        // Deletes: removed_resources contains full resource paths (kind/scopeQualifier/name)
        for (String removedPath : response.getRemovedResourcesList()) {
            ConfigResourceId resourceId = ConfigResourceId.fromPath(removedPath);
            mutations.add(new ConfigMutation.Delete(resourceId));
        }

        return mutations;
    }

    @Override
    public void close() {
        running = false;
        StreamObserver<ConfigRequest> observer = requestObserver;
        if (observer != null) {
            try {
                synchronized (writeLock) {
                    observer.onCompleted();
                }
            } catch (Exception ignored) {
                // Stream may already be closed
            }
        }
    }
}
