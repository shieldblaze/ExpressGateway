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
package com.shieldblaze.expressgateway.controlplane.registry;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.v1.NodeIdentity;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * A persistent wrapper around {@link NodeRegistry} that persists node registration
 * data to a {@link KVStore} for crash recovery and cross-instance visibility.
 *
 * <h3>KV Store layout</h3>
 * <pre>
 * /expressgateway/nodes/{nodeId}   -- JSON-serialized NodeRegistrationRecord
 * </pre>
 *
 * <h3>Design</h3>
 * <p>Wraps (delegates to) an existing {@link NodeRegistry} for in-memory operations.
 * On registration, writes the node record to the KV store. On deregistration, deletes
 * it. Heartbeat-driven config version updates are coalesced: a background flush thread
 * batches dirty nodes and writes them to the KV store at a configurable interval,
 * preventing per-heartbeat KV store writes which would overwhelm the backend.</p>
 *
 * <p>KV store write failures are non-fatal for registration operations. A warning is
 * logged but the in-memory registration proceeds. This prevents KV store transient
 * failures from blocking node connectivity.</p>
 *
 * <p>On startup, {@link #loadFromKVStore()} can be called to restore previously
 * persisted node records. This is the crash recovery path. Note: restored nodes
 * will be in CONNECTED state and will need to re-establish heartbeat streams.</p>
 *
 * <p>Thread safety: all KV store writes happen on the flush thread or inline on the
 * caller's thread (register/deregister). The dirty set uses {@link ConcurrentHashMap}
 * for lock-free concurrent access.</p>
 */
@Log4j2
public final class PersistentNodeRegistry implements Closeable {

    private static final String NODES_PREFIX = "/expressgateway/nodes/";
    private static final ObjectMapper MAPPER = new ObjectMapper();

    static {
        MAPPER.findAndRegisterModules();
    }

    /** Default coalescing flush interval (5 seconds). */
    private static final long DEFAULT_FLUSH_INTERVAL_MS = 5000L;

    private final NodeRegistry delegate;
    private final KVStore kvStore;
    private final long flushIntervalMs;
    private final ScheduledExecutorService flushScheduler;
    private final AtomicBoolean running = new AtomicBoolean(false);

    /**
     * Set of node IDs whose {@code appliedConfigVersion} has been updated since the
     * last flush. The flush thread drains this set and writes the current version
     * for each dirty node to the KV store.
     */
    private final ConcurrentHashMap.KeySetView<String, Boolean> dirtyNodes = ConcurrentHashMap.newKeySet();

    /**
     * Creates a PersistentNodeRegistry wrapping the given in-memory registry.
     *
     * @param delegate        the underlying in-memory node registry; must not be null
     * @param kvStore         the KV store for persistence; must not be null
     * @param flushIntervalMs interval between coalesced heartbeat flushes in ms; must be >= 100
     */
    public PersistentNodeRegistry(NodeRegistry delegate, KVStore kvStore, long flushIntervalMs) {
        this.delegate = Objects.requireNonNull(delegate, "delegate");
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        if (flushIntervalMs < 100) {
            throw new IllegalArgumentException("flushIntervalMs must be >= 100, got: " + flushIntervalMs);
        }
        this.flushIntervalMs = flushIntervalMs;
        this.flushScheduler = Executors.newSingleThreadScheduledExecutor(r -> {
            Thread t = new Thread(r, "persistent-node-registry-flush");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Convenience constructor using the default flush interval (5 seconds).
     */
    public PersistentNodeRegistry(NodeRegistry delegate, KVStore kvStore) {
        this(delegate, kvStore, DEFAULT_FLUSH_INTERVAL_MS);
    }

    /**
     * Starts the coalescing flush thread. Must be called once before use.
     */
    public void start() {
        if (!running.compareAndSet(false, true)) {
            throw new IllegalStateException("PersistentNodeRegistry is already running");
        }
        flushScheduler.scheduleAtFixedRate(this::flushDirtyNodes,
                flushIntervalMs, flushIntervalMs, TimeUnit.MILLISECONDS);
        log.info("PersistentNodeRegistry started: flushInterval={}ms", flushIntervalMs);
    }

    /**
     * Returns the underlying in-memory {@link NodeRegistry}.
     */
    public NodeRegistry delegate() {
        return delegate;
    }

    /**
     * Registers a node in both the in-memory registry and the KV store.
     *
     * <p>The KV store write is best-effort: if it fails, the in-memory registration
     * still succeeds and a warning is logged.</p>
     *
     * @param identity     the node identity; must not be null
     * @param sessionToken the session token; must not be null
     * @return the newly registered {@link DataPlaneNode}
     */
    public DataPlaneNode register(NodeIdentity identity, String sessionToken) {
        DataPlaneNode node = delegate.register(identity, sessionToken);

        // Persist to KV store (best-effort)
        try {
            NodeRegistrationRecord record = NodeRegistrationRecord.from(node);
            byte[] serialized = MAPPER.writeValueAsBytes(record);
            kvStore.put(nodeKey(node.nodeId()), serialized);
            log.debug("Persisted node registration to KV store: nodeId={}", node.nodeId());
        } catch (Exception e) {
            log.warn("Failed to persist node registration to KV store: nodeId={}", node.nodeId(), e);
        }

        return node;
    }

    /**
     * Deregisters a node from both the in-memory registry and the KV store.
     *
     * @param nodeId the node ID to deregister; must not be null
     * @return the removed node, or null if not found
     */
    public DataPlaneNode deregister(String nodeId) {
        DataPlaneNode removed = delegate.deregister(nodeId);
        dirtyNodes.remove(nodeId);

        if (removed != null) {
            // Delete from KV store (best-effort)
            try {
                kvStore.delete(nodeKey(nodeId));
                log.debug("Removed node registration from KV store: nodeId={}", nodeId);
            } catch (KVStoreException e) {
                log.warn("Failed to remove node registration from KV store: nodeId={}", nodeId, e);
            }
        }

        return removed;
    }

    /**
     * Marks a node's config version as dirty, so the next flush cycle will
     * persist it to the KV store. Call this when a heartbeat updates the
     * node's {@code appliedConfigVersion}.
     *
     * <p>This is the coalescing buffer entry point: multiple heartbeats between
     * flush intervals are collapsed into a single KV write.</p>
     *
     * @param nodeId the node whose config version was updated
     */
    public void markDirty(String nodeId) {
        dirtyNodes.add(nodeId);
    }

    /**
     * Loads previously persisted node registrations from the KV store into
     * the in-memory registry. This is the crash recovery path.
     *
     * <p>Nodes are restored in CONNECTED state. They will need to re-establish
     * heartbeat and config streams to be considered operational.</p>
     *
     * @return the number of nodes loaded
     * @throws KVStoreException if the KV store list operation fails
     */
    public int loadFromKVStore() throws KVStoreException {
        List<KVEntry> entries = kvStore.list(NODES_PREFIX);
        int loaded = 0;
        for (KVEntry entry : entries) {
            try {
                NodeRegistrationRecord record = MAPPER.readValue(entry.value(), NodeRegistrationRecord.class);
                // Check if already registered (e.g., the node reconnected before recovery)
                if (delegate.get(record.nodeId()).isPresent()) {
                    log.debug("Skipping KV recovery for already-registered node: {}", record.nodeId());
                    continue;
                }
                // Build a NodeIdentity proto from the persisted record
                NodeIdentity.Builder identityBuilder = NodeIdentity.newBuilder()
                        .setNodeId(record.nodeId())
                        .setClusterId(record.clusterId() != null ? record.clusterId() : "")
                        .setEnvironment(record.environment() != null ? record.environment() : "")
                        .setAddress(record.address() != null ? record.address() : "")
                        .setBuildVersion(record.buildVersion() != null ? record.buildVersion() : "");
                if (record.metadata() != null) {
                    identityBuilder.putAllMetadata(record.metadata());
                }
                delegate.register(identityBuilder.build(), record.sessionToken());
                loaded++;
                log.info("Recovered node from KV store: nodeId={}", record.nodeId());
            } catch (IOException e) {
                log.warn("Failed to deserialize node registration record at key={}", entry.key(), e);
            } catch (IllegalStateException e) {
                // Node already registered (race condition)
                log.debug("Node already registered during recovery: {}", e.getMessage());
            }
        }
        log.info("Loaded {} node registrations from KV store", loaded);
        return loaded;
    }

    /**
     * Flushes dirty node records to the KV store. Called periodically by the
     * flush scheduler. Each dirty node's current state is serialized and written.
     */
    private void flushDirtyNodes() {
        try {
            if (dirtyNodes.isEmpty()) {
                return;
            }

            // Snapshot and clear the dirty set atomically per-node.
            // We iterate and remove one-by-one to avoid missing nodes that become
            // dirty during the flush.
            for (String nodeId : dirtyNodes) {
                if (!dirtyNodes.remove(nodeId)) {
                    continue; // Already flushed by concurrent removal
                }

                var nodeOpt = delegate.get(nodeId);
                if (nodeOpt.isEmpty()) {
                    continue; // Node was deregistered between marking dirty and flushing
                }

                DataPlaneNode node = nodeOpt.get();
                try {
                    NodeRegistrationRecord record = NodeRegistrationRecord.from(node);
                    byte[] serialized = MAPPER.writeValueAsBytes(record);
                    kvStore.put(nodeKey(nodeId), serialized);
                } catch (Exception e) {
                    log.warn("Failed to flush node registration to KV store: nodeId={}", nodeId, e);
                    // Re-add to dirty set so it's retried on the next flush
                    dirtyNodes.add(nodeId);
                }
            }
        } catch (Exception e) {
            log.error("Error during dirty node flush", e);
        }
    }

    @Override
    public void close() {
        running.set(false);
        // Perform one final flush before shutting down
        flushDirtyNodes();
        flushScheduler.shutdown();
        try {
            if (!flushScheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                flushScheduler.shutdownNow();
                log.warn("PersistentNodeRegistry flush scheduler did not terminate within 5 seconds");
            }
        } catch (InterruptedException e) {
            flushScheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }
        log.info("PersistentNodeRegistry stopped");
    }

    private static String nodeKey(String nodeId) {
        return NODES_PREFIX + nodeId;
    }

    /**
     * Serializable record for persisting node registration data to the KV store.
     *
     * <p>Contains all fields needed to reconstruct a {@link DataPlaneNode} on recovery,
     * plus the session token needed for session validation.</p>
     */
    public record NodeRegistrationRecord(
            @JsonProperty("nodeId") String nodeId,
            @JsonProperty("sessionToken") String sessionToken,
            @JsonProperty("clusterId") String clusterId,
            @JsonProperty("environment") String environment,
            @JsonProperty("address") String address,
            @JsonProperty("buildVersion") String buildVersion,
            @JsonProperty("appliedConfigVersion") long appliedConfigVersion,
            @JsonProperty("region") String region,
            @JsonProperty("metadata") Map<String, String> metadata,
            @JsonProperty("connectedAt") Instant connectedAt,
            @JsonProperty("state") String state
    ) {
        /**
         * Creates a record from a live {@link DataPlaneNode}.
         */
        static NodeRegistrationRecord from(DataPlaneNode node) {
            return new NodeRegistrationRecord(
                    node.nodeId(),
                    node.sessionToken(),
                    node.clusterId(),
                    node.environment(),
                    node.address(),
                    node.buildVersion(),
                    node.appliedConfigVersion(),
                    null, // region is not tracked on DataPlaneNode; assigned by CP cluster logic
                    node.metadata(),
                    node.connectedAt(),
                    node.state().name()
            );
        }
    }
}
