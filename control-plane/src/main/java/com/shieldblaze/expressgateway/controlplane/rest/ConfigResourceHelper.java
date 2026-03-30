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
package com.shieldblaze.expressgateway.controlplane.rest;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.cluster.ConfigWriteForwarder;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKindRegistration;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKindRegistry;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;

import java.io.IOException;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;

/**
 * Shared utility for CRUD controllers that manage {@link ConfigResource} lifecycle.
 *
 * <p>Encapsulates the pattern: DTO -> ConfigSpec -> ConfigResource -> KVStore persist -> ConfigDistributor submit.
 * All controllers delegate to this helper to avoid duplicating serialization, KV key construction,
 * and mutation submission logic.</p>
 */
final class ConfigResourceHelper {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final String KV_PREFIX = "/expressgateway/config";
    private static final String REST_AUTHOR = "control-plane-rest-api";

    static {
        MAPPER.findAndRegisterModules();
    }

    private ConfigResourceHelper() {
        // utility class
    }

    /**
     * Build a {@link ConfigResource} envelope from a kind, name, spec, and optional labels.
     *
     * @param kind   the config kind
     * @param name   the resource name
     * @param spec   the typed spec payload
     * @param labels optional labels (may be null)
     * @return a new {@link ConfigResource} at version 1
     */
    static ConfigResource createResource(ConfigKind kind, String name, ConfigSpec spec, Map<String, String> labels) {
        Instant now = Instant.now();
        return new ConfigResource(
                new ConfigResourceId(kind.name(), "global", name),
                kind,
                new ConfigScope.Global(),
                1L,
                now,
                now,
                REST_AUTHOR,
                labels != null ? labels : Map.of(),
                spec
        );
    }

    /**
     * Build a {@link ConfigResource} envelope with an explicit scope qualifier.
     *
     * <p>If {@code scopeQualifier} is null, blank, or {@code "global"}, the resource
     * is created with {@link ConfigScope.Global}. Otherwise, the qualifier is parsed
     * via {@link ConfigScope#fromQualifier(String)} to produce the appropriate scope.</p>
     *
     * @param kind           the config kind
     * @param name           the resource name
     * @param spec           the typed spec payload
     * @param labels         optional labels (may be null)
     * @param scopeQualifier optional scope qualifier string (may be null for global)
     * @return a new {@link ConfigResource} at version 1
     */
    static ConfigResource createResource(ConfigKind kind, String name, ConfigSpec spec,
                                          Map<String, String> labels, String scopeQualifier) {
        ConfigScope scope = (scopeQualifier == null || scopeQualifier.isBlank() || "global".equals(scopeQualifier))
                ? new ConfigScope.Global()
                : ConfigScope.fromQualifier(scopeQualifier);
        String qualifier = scope.qualifier();
        Instant now = Instant.now();
        return new ConfigResource(
                new ConfigResourceId(kind.name(), qualifier, name),
                kind,
                scope,
                1L,
                now,
                now,
                REST_AUTHOR,
                labels != null ? labels : Map.of(),
                spec
        );
    }

    /**
     * Build a {@link ConfigResource} for an update, incrementing the version from the existing resource.
     *
     * @param existing the existing resource being updated
     * @param spec     the new spec payload
     * @param labels   optional labels (may be null)
     * @return a new {@link ConfigResource} with incremented version
     */
    static ConfigResource updateResource(ConfigResource existing, ConfigSpec spec, Map<String, String> labels) {
        return new ConfigResource(
                existing.id(),
                existing.kind(),
                existing.scope(),
                existing.version() + 1,
                existing.createdAt(),
                Instant.now(),
                REST_AUTHOR,
                labels != null ? labels : existing.labels(),
                spec
        );
    }

    /**
     * Serialize a {@link ConfigResource} to JSON bytes and persist to the KV store,
     * then submit an {@link ConfigMutation.Upsert} to the distributor for propagation.
     *
     * @param resource    the resource to persist
     * @param kvStore     the KV store backend
     * @param distributor the config distributor
     * @throws KVStoreException        if the KV store write fails
     * @throws JsonProcessingException if serialization fails
     */
    static void persistAndDistribute(ConfigResource resource, KVStore kvStore, ConfigDistributor distributor)
            throws KVStoreException, JsonProcessingException {
        validateSpec(resource.kind(), resource.spec());
        byte[] serialized = MAPPER.writeValueAsBytes(resource);
        String key = kvKey(resource.id());
        kvStore.put(key, serialized);
        distributor.submit(new ConfigMutation.Upsert(resource));
    }

    /**
     * Look up the {@link ConfigKindRegistration} for the given kind and invoke its
     * {@link ConfigKindRegistration#validator()} against the spec. If validation fails,
     * throws {@link IllegalArgumentException} with all error messages joined.
     *
     * <p>If no registration exists for the kind (e.g. dynamically created kinds without
     * a provider on the classpath), validation is skipped -- the resource will be persisted
     * as-is. This is a conscious choice: unknown kinds are allowed to flow through the
     * store but will not receive validation protection.</p>
     *
     * @param kind the config kind to look up
     * @param spec the spec payload to validate
     * @throws IllegalArgumentException if validation produces errors
     */
    static void validateSpec(ConfigKind kind, ConfigSpec spec) {
        Optional<ConfigKindRegistration> reg = ConfigKindRegistry.get(kind.name());
        if (reg.isPresent()) {
            List<String> errors = reg.get().validator().validate(spec);
            if (!errors.isEmpty()) {
                throw new IllegalArgumentException("Config validation failed: " + String.join("; ", errors));
            }
        }
    }

    /**
     * Persist and distribute with leader awareness. If clustering is enabled and this
     * instance is a follower, the mutation is forwarded to the leader via the
     * {@link ConfigWriteForwarder} (KV-based pending-mutation queue). If this instance
     * is the leader or clustering is disabled, the mutation is processed locally.
     *
     * <p>This prevents split-brain: followers never write directly to the KV store
     * or the leader's ChangeJournal. All writes go through the single-writer leader.</p>
     *
     * @param resource    the resource to persist
     * @param kvStore     the KV store backend
     * @param distributor the config distributor
     * @param cluster     the cluster manager (null when clustering is disabled)
     * @param forwarder   the write forwarder (null when clustering is disabled)
     * @throws KVStoreException        if persistence or forwarding fails
     * @throws JsonProcessingException if serialization fails
     */
    static void persistAndDistributeWithLeaderCheck(
            ConfigResource resource, KVStore kvStore, ConfigDistributor distributor,
            ControlPlaneCluster cluster, ConfigWriteForwarder forwarder)
            throws KVStoreException, JsonProcessingException {
        if (cluster != null && !cluster.isLeader()) {
            Objects.requireNonNull(forwarder, "forwarder must not be null when cluster is non-null");
            try {
                forwarder.forwardMutation(new ConfigMutation.Upsert(resource)).get(30, TimeUnit.SECONDS);
            } catch (KVStoreException e) {
                throw e;
            } catch (ExecutionException e) {
                Throwable cause = e.getCause();
                if (cause instanceof KVStoreException kve) {
                    throw kve;
                }
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to forward mutation to leader: " + cause.getMessage(), cause);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Interrupted while forwarding mutation to leader", e);
            } catch (TimeoutException e) {
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Timed out forwarding mutation to leader (30s)", e);
            }
            return;
        }
        // We're the leader (or single-node) -- persist locally
        persistAndDistribute(resource, kvStore, distributor);
    }

    /**
     * Delete and distribute with leader awareness. If clustering is enabled and this
     * instance is a follower, the deletion is forwarded to the leader via the
     * {@link ConfigWriteForwarder}. If this instance is the leader or clustering is
     * disabled, the deletion is processed locally.
     *
     * @param id          the resource ID to delete
     * @param kvStore     the KV store backend
     * @param distributor the config distributor
     * @param cluster     the cluster manager (null when clustering is disabled)
     * @param forwarder   the write forwarder (null when clustering is disabled)
     * @throws KVStoreException if persistence or forwarding fails
     */
    static void deleteAndDistributeWithLeaderCheck(
            ConfigResourceId id, KVStore kvStore, ConfigDistributor distributor,
            ControlPlaneCluster cluster, ConfigWriteForwarder forwarder)
            throws KVStoreException {
        if (cluster != null && !cluster.isLeader()) {
            Objects.requireNonNull(forwarder, "forwarder must not be null when cluster is non-null");
            try {
                forwarder.forwardMutation(new ConfigMutation.Delete(id)).get(30, TimeUnit.SECONDS);
            } catch (KVStoreException e) {
                throw e;
            } catch (ExecutionException e) {
                Throwable cause = e.getCause();
                if (cause instanceof KVStoreException kve) {
                    throw kve;
                }
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to forward deletion to leader: " + cause.getMessage(), cause);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Interrupted while forwarding deletion to leader", e);
            } catch (TimeoutException e) {
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Timed out forwarding deletion to leader (30s)", e);
            }
            return;
        }
        // We're the leader (or single-node) -- delete locally
        deleteAndDistribute(id, kvStore, distributor);
    }

    /**
     * Delete a resource from the KV store and submit a {@link ConfigMutation.Delete}
     * to the distributor.
     *
     * @param id          the resource ID to delete
     * @param kvStore     the KV store backend
     * @param distributor the config distributor
     * @throws KVStoreException if the KV store delete fails
     */
    static void deleteAndDistribute(ConfigResourceId id, KVStore kvStore, ConfigDistributor distributor)
            throws KVStoreException {
        String key = kvKey(id);
        boolean deleted = kvStore.delete(key);
        if (!deleted) {
            throw new KVStoreException(KVStoreException.Code.KEY_NOT_FOUND, "Resource not found: " + id.toPath());
        }
        distributor.submit(new ConfigMutation.Delete(id));
    }

    /**
     * Retrieve a single {@link ConfigResource} from the KV store.
     *
     * @param id      the resource ID
     * @param kvStore the KV store backend
     * @return the deserialized resource, or empty if not found
     * @throws KVStoreException if the KV store read fails
     * @throws IOException      if deserialization fails
     */
    static Optional<ConfigResource> getResource(ConfigResourceId id, KVStore kvStore)
            throws KVStoreException, IOException {
        String key = kvKey(id);
        Optional<KVEntry> entry = kvStore.get(key);
        if (entry.isEmpty()) {
            return Optional.empty();
        }
        ConfigResource resource = MAPPER.readValue(entry.get().value(), ConfigResource.class);
        return Optional.of(resource);
    }

    /**
     * List all {@link ConfigResource} entries under a given config kind prefix.
     *
     * @param kind    the config kind to list
     * @param kvStore the KV store backend
     * @return list of deserialized resources (never null, may be empty)
     * @throws KVStoreException if the KV store read fails
     */
    static List<ConfigResource> listResources(ConfigKind kind, KVStore kvStore) throws KVStoreException {
        return listResources(kind, "global", kvStore);
    }

    /**
     * List all {@link ConfigResource} entries under a given config kind and scope qualifier prefix.
     *
     * @param kind           the config kind to list
     * @param scopeQualifier the scope qualifier (e.g. "global", "cluster:prod-1")
     * @param kvStore        the KV store backend
     * @return list of deserialized resources (never null, may be empty)
     * @throws KVStoreException if the KV store read fails
     */
    static List<ConfigResource> listResources(ConfigKind kind, String scopeQualifier, KVStore kvStore) throws KVStoreException {
        String prefix = KV_PREFIX + "/" + kind.name() + "/" + scopeQualifier;
        List<KVEntry> entries = kvStore.list(prefix);
        List<ConfigResource> resources = new ArrayList<>(entries.size());
        for (KVEntry entry : entries) {
            try {
                resources.add(MAPPER.readValue(entry.value(), ConfigResource.class));
            } catch (IOException e) {
                // Skip malformed entries rather than failing the entire list operation.
                // In production this should emit a metric for monitoring.
            }
        }
        return Collections.unmodifiableList(resources);
    }

    /**
     * Construct the KV store key for a given {@link ConfigResourceId}.
     */
    static String kvKey(ConfigResourceId id) {
        Objects.requireNonNull(id, "id");
        return KV_PREFIX + "/" + id.toPath();
    }
}
