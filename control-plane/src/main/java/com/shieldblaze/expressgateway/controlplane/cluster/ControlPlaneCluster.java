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
package com.shieldblaze.expressgateway.controlplane.cluster;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.shieldblaze.expressgateway.controlplane.ControlPlaneConfiguration;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatchEvent;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.Collection;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * Manages the cluster of control plane instances using a shared {@link KVStore}
 * for coordination, discovery, and leader election.
 *
 * <h3>Responsibilities</h3>
 * <ul>
 *   <li>Register this CP instance in the KV store on startup</li>
 *   <li>Discover peer CP instances via KV watch on the instances prefix</li>
 *   <li>Leader election using {@link KVStore#leaderElection(String, String)}</li>
 *   <li>Periodic heartbeat to maintain registration (every 10 seconds)</li>
 *   <li>Background stale-peer eviction (peers not heartbeated in 60 seconds)</li>
 * </ul>
 *
 * <h3>KV Store layout</h3>
 * <pre>
 * /expressgateway/controlplane/instances/{instanceId}  -- JSON-serialized ControlPlaneInstance
 * /expressgateway/controlplane/leader                  -- leader election path
 * </pre>
 *
 * <h3>Design rationale</h3>
 * <p>Leader-based with follower forwarding. The KV store (etcd/Consul/ZooKeeper) provides
 * linearizable writes and distributed coordination, so no additional consensus protocol
 * is needed. All CP instances serve data-plane gRPC connections independently; only
 * config writes are routed through the leader.</p>
 *
 * <p>Thread safety: peer map uses {@link ConcurrentHashMap}; leadership flag is volatile.
 * The heartbeat and stale-peer scanner run on dedicated daemon threads.</p>
 */
@Log4j2
public final class ControlPlaneCluster implements Closeable {

    private static final String INSTANCES_PREFIX = "/expressgateway/controlplane/instances/";
    private static final String LEADER_ELECTION_PATH = "/expressgateway/controlplane/leader";
    private static final String PENDING_MUTATIONS_PREFIX = "/expressgateway/controlplane/pending-mutations/";
    private static final String MUTATION_RESULTS_PREFIX = "/expressgateway/controlplane/mutation-results/";
    private static final ObjectMapper MAPPER = new ObjectMapper();

    static {
        MAPPER.findAndRegisterModules();
    }

    /** Heartbeat interval for updating own registration in the KV store. */
    private static final long HEARTBEAT_INTERVAL_SECONDS = 10;

    /** Peers that haven't heartbeated within this window are considered stale. */
    private static final long STALE_PEER_THRESHOLD_MS = 60_000;

    /** How often to scan for stale peers. */
    private static final long STALE_PEER_SCAN_INTERVAL_SECONDS = 30;

    /** Time-to-live for processed nonce entries in the dedup map (5 minutes). */
    private static final long NONCE_EXPIRY_MS = 5 * 60 * 1000L;

    /** How often to clean expired nonces from the dedup map (60 seconds). */
    private static final long NONCE_CLEANUP_INTERVAL_SECONDS = 60;

    private final KVStore kvStore;
    private final ControlPlaneConfiguration config;
    private final String instanceId;
    private final String region;
    private final ConcurrentHashMap<String, ControlPlaneInstance> peers = new ConcurrentHashMap<>();
    private final ScheduledExecutorService scheduler;

    private volatile boolean isLeader;
    private volatile KVStore.LeaderElection leaderElection;
    private volatile Closeable instanceWatch;
    private volatile Closeable pendingMutationsWatch;
    private volatile ConfigDistributor configDistributor;
    private volatile ConcurrentHashMap<String, Long> processedNonceMap;
    private volatile java.util.concurrent.ScheduledFuture<?> nonceCleanupFuture;

    /**
     * Creates a new cluster manager.
     *
     * @param kvStore    the KV store used for coordination; must not be null and must be connected
     * @param config     the control plane configuration; must not be null
     * @param instanceId unique identifier for this CP instance; must not be null or blank
     * @param region     the geographic region of this instance; must not be null or blank
     */
    public ControlPlaneCluster(KVStore kvStore, ControlPlaneConfiguration config,
                               String instanceId, String region) {
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        this.config = Objects.requireNonNull(config, "config");
        this.instanceId = Objects.requireNonNull(instanceId, "instanceId");
        this.region = Objects.requireNonNull(region, "region");

        if (instanceId.isBlank()) {
            throw new IllegalArgumentException("instanceId must not be blank");
        }
        if (region.isBlank()) {
            throw new IllegalArgumentException("region must not be blank");
        }

        this.scheduler = Executors.newScheduledThreadPool(2, r -> {
            Thread t = new Thread(r, "cp-cluster-" + instanceId.substring(0, Math.min(8, instanceId.length())));
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Sets the {@link ConfigDistributor} used to process pending mutations forwarded
     * by follower instances. This must be called after construction because the
     * distributor is created before the cluster in the {@code ControlPlaneServer}
     * wiring sequence.
     *
     * <p>If this instance is already the leader when the distributor is set, any
     * existing pending mutations in the KV store will be drained immediately and
     * a watch will be installed for new ones.</p>
     *
     * @param distributor the config distributor; must not be null
     */
    public void setConfigDistributor(ConfigDistributor distributor) {
        this.configDistributor = Objects.requireNonNull(distributor, "distributor");
        log.info("ConfigDistributor set on ControlPlaneCluster");

        // If we are already leader (e.g. initial election completed before this setter was called),
        // install the pending-mutations watch now.
        if (isLeader && pendingMutationsWatch == null) {
            installPendingMutationsWatch();
        }
    }

    /**
     * Starts cluster participation: registers this instance, begins leader election,
     * installs a watch for peer changes, and schedules heartbeat and stale-peer scans.
     *
     * <p>Must be called exactly once. Calling multiple times will result in duplicate
     * registrations.</p>
     *
     * @throws KVStoreException if registration, election setup, or watch installation fails
     */
    public void start() throws KVStoreException {
        log.info("Starting ControlPlaneCluster: instanceId={}, region={}", instanceId, region);

        // 1. Register self
        registerSelf();

        // 2. Load existing peers before starting the watch to avoid a race where
        //    we miss peers that were registered before the watch was installed.
        loadExistingPeers();

        // 3. Watch for peer changes (additions, heartbeat updates, removals)
        instanceWatch = kvStore.watch(INSTANCES_PREFIX, this::onPeerEvent);

        // 4. Start leader election
        leaderElection = kvStore.leaderElection(LEADER_ELECTION_PATH, instanceId);
        leaderElection.addListener(this::onLeaderChange);
        // Capture initial state -- addListener may not fire for the initial value
        isLeader = leaderElection.isLeader();
        log.info("Initial leadership status: isLeader={}", isLeader);

        // If we are already the leader and the distributor is wired, install the
        // pending-mutations watch. This covers the case where addListener did NOT
        // fire for the initial state (depends on the KV store implementation).
        if (isLeader && configDistributor != null && pendingMutationsWatch == null) {
            installPendingMutationsWatch();
        }

        // 5. Schedule periodic heartbeat
        scheduler.scheduleAtFixedRate(this::heartbeat,
                HEARTBEAT_INTERVAL_SECONDS, HEARTBEAT_INTERVAL_SECONDS, TimeUnit.SECONDS);

        // 6. Schedule stale-peer scanner
        scheduler.scheduleAtFixedRate(this::scanStalePeers,
                STALE_PEER_SCAN_INTERVAL_SECONDS, STALE_PEER_SCAN_INTERVAL_SECONDS, TimeUnit.SECONDS);

        log.info("ControlPlaneCluster started: {} peers discovered", peers.size());
    }

    /**
     * Returns whether this instance currently holds the leader role.
     *
     * @return {@code true} if this instance is the leader
     */
    public boolean isLeader() {
        return isLeader;
    }

    /**
     * Returns the current leader instance, if known.
     *
     * <p>The leader might be this instance or a peer. If no leader is elected
     * (transient state during election), returns empty.</p>
     *
     * <p>Uses {@link KVStore.LeaderElection#currentLeaderId()} to resolve the
     * leader's participantId, then looks up the corresponding
     * {@link ControlPlaneInstance} from the peers map. If the leader ID is not
     * found in the local peers map (e.g., the peer registration event has not
     * yet been processed), returns empty and logs a warning.</p>
     *
     * @return the current leader, or empty if no leader is available
     */
    public Optional<ControlPlaneInstance> currentLeader() {
        KVStore.LeaderElection election = this.leaderElection;
        if (election == null) {
            // Election not yet initialized (start() hasn't been called)
            return Optional.empty();
        }

        try {
            String leaderId = election.currentLeaderId();
            ControlPlaneInstance leader = peers.get(leaderId);
            if (leader == null) {
                // The leader's registration may not have been processed by our watch yet,
                // or the peer may have been evicted by the stale-peer scanner.
                log.warn("Leader participantId={} resolved from election but not found in peers map " +
                        "(peer count={}). This is transient during election or peer discovery.",
                        leaderId, peers.size());
                return Optional.empty();
            }
            return Optional.of(leader);
        } catch (KVStoreException e) {
            // This can happen during election transitions, network partitions,
            // or if no leader has been elected yet.
            log.debug("Failed to resolve current leader from election: {}", e.getMessage());
            return Optional.empty();
        }
    }

    /**
     * Returns an unmodifiable view of all known peer instances (including self).
     *
     * @return collection of all registered control plane instances
     */
    public Collection<ControlPlaneInstance> peers() {
        return Collections.unmodifiableCollection(peers.values());
    }

    /**
     * Returns the unique instance ID of this control plane instance.
     *
     * @return the instance ID
     */
    public String instanceId() {
        return instanceId;
    }

    /**
     * Returns the region of this control plane instance.
     *
     * @return the region string
     */
    public String region() {
        return region;
    }

    /**
     * Registers this instance in the KV store at
     * {@code /expressgateway/controlplane/instances/{instanceId}}.
     */
    private void registerSelf() throws KVStoreException {
        long now = System.currentTimeMillis();
        ControlPlaneInstance self = new ControlPlaneInstance(
                instanceId, region, config.grpcBindAddress(), config.grpcPort(), now, now);

        byte[] serialized = serializeInstance(self);
        kvStore.put(instanceKey(instanceId), serialized);
        peers.put(instanceId, self);

        log.info("Registered self in KV store: instanceId={}, region={}, grpcAddress={}:{}",
                instanceId, region, config.grpcBindAddress(), config.grpcPort());
    }

    /**
     * Loads all existing peer registrations from the KV store into the local peer map.
     * Called once during startup before the watch is installed.
     */
    private void loadExistingPeers() throws KVStoreException {
        List<KVEntry> entries = kvStore.list(INSTANCES_PREFIX);
        for (KVEntry entry : entries) {
            try {
                ControlPlaneInstance peer = MAPPER.readValue(entry.value(), ControlPlaneInstance.class);
                peers.put(peer.instanceId(), peer);
            } catch (IOException e) {
                log.warn("Failed to deserialize peer entry at key={}", entry.key(), e);
            }
        }
    }

    /**
     * Watch callback: processes peer registration, update, and removal events.
     */
    private void onPeerEvent(KVWatchEvent event) {
        try {
            switch (event.type()) {
                case PUT -> {
                    if (event.entry() != null) {
                        ControlPlaneInstance peer = MAPPER.readValue(event.entry().value(), ControlPlaneInstance.class);
                        ControlPlaneInstance previous = peers.put(peer.instanceId(), peer);
                        if (previous == null) {
                            log.info("New peer discovered: instanceId={}, region={}", peer.instanceId(), peer.region());
                        }
                    }
                }
                case DELETE -> {
                    // Extract instanceId from the key: /expressgateway/controlplane/instances/{instanceId}
                    if (event.entry() != null) {
                        String key = event.entry().key();
                        String removedId = key.substring(INSTANCES_PREFIX.length());
                        ControlPlaneInstance removed = peers.remove(removedId);
                        if (removed != null) {
                            log.info("Peer removed: instanceId={}, region={}", removed.instanceId(), removed.region());
                        }
                    } else if (event.previousEntry() != null) {
                        String key = event.previousEntry().key();
                        String removedId = key.substring(INSTANCES_PREFIX.length());
                        ControlPlaneInstance removed = peers.remove(removedId);
                        if (removed != null) {
                            log.info("Peer removed: instanceId={}, region={}", removed.instanceId(), removed.region());
                        }
                    }
                }
            }
        } catch (IOException e) {
            log.error("Failed to process peer watch event", e);
        }
    }

    /**
     * Leadership change callback. When becoming leader, installs a watch on the
     * pending-mutations prefix so that mutations forwarded by followers are processed.
     * When losing leadership, the watch is closed to stop processing.
     */
    private void onLeaderChange(boolean nowLeader) {
        boolean was = this.isLeader;
        this.isLeader = nowLeader;

        if (nowLeader && !was) {
            log.info("This instance ({}) has become the LEADER", instanceId);
            // Only install the watch if the distributor has been wired.
            // If not yet set, setConfigDistributor() will install it.
            if (configDistributor != null && pendingMutationsWatch == null) {
                installPendingMutationsWatch();
            }
        } else if (!nowLeader && was) {
            log.warn("This instance ({}) has LOST leadership", instanceId);
            closePendingMutationsWatch();
        }
    }

    /**
     * Installs a KV watch on the pending-mutations prefix and drains any existing
     * entries that were written before the watch was installed. A processed-nonce set
     * is used to deduplicate between the watch callback and the initial list scan,
     * since a PUT event may fire for an entry that is also returned by the list call.
     */
    private void installPendingMutationsWatch() {
        ConfigDistributor distributor = this.configDistributor;
        if (distributor == null) {
            log.warn("Cannot install pending-mutations watch: ConfigDistributor not set");
            return;
        }

        // Track nonces that have already been processed to avoid double-submission
        // between the watch callback and the initial list drain.
        //
        // Bounded: uses ConcurrentHashMap<String, Long> (nonce -> processingTimestamp).
        // Entries older than NONCE_EXPIRY_MS are evicted periodically by the scheduler
        // to prevent unbounded growth under sustained mutation traffic.
        ConcurrentHashMap<String, Long> processedNonces = new ConcurrentHashMap<>();
        this.processedNonceMap = processedNonces;

        // Schedule periodic cleanup of stale nonces (every 60 seconds).
        // Track the future so closePendingMutationsWatch() can cancel it on leadership loss,
        // preventing accumulation of cleanup tasks under rapid leadership flips.
        nonceCleanupFuture = scheduler.scheduleAtFixedRate(() -> {
            try {
                long cutoff = System.currentTimeMillis() - NONCE_EXPIRY_MS;
                processedNonces.entrySet().removeIf(e -> e.getValue() < cutoff);
            } catch (Exception e) {
                log.error("Error during processed-nonce cleanup", e);
            }
        }, NONCE_CLEANUP_INTERVAL_SECONDS, NONCE_CLEANUP_INTERVAL_SECONDS, TimeUnit.SECONDS);

        try {
            // 1. Install the watch FIRST so we don't miss mutations written after the list scan.
            pendingMutationsWatch = kvStore.watch(PENDING_MUTATIONS_PREFIX, event -> {
                if (event.type() == KVWatchEvent.Type.PUT && event.entry() != null) {
                    String key = event.entry().key();
                    String nonce = key.substring(PENDING_MUTATIONS_PREFIX.length());
                    if (processedNonces.putIfAbsent(nonce, System.currentTimeMillis()) == null) {
                        processPendingMutation(nonce, event.entry().value(), distributor);
                    }
                }
            });
            log.info("Installed pending-mutations watch on {}", PENDING_MUTATIONS_PREFIX);

            // 2. Drain any existing pending mutations that were written before the watch was active.
            List<KVEntry> existing = kvStore.list(PENDING_MUTATIONS_PREFIX);
            if (!existing.isEmpty()) {
                log.info("Draining {} existing pending mutation(s)", existing.size());
                for (KVEntry entry : existing) {
                    String nonce = entry.key().substring(PENDING_MUTATIONS_PREFIX.length());
                    if (processedNonces.putIfAbsent(nonce, System.currentTimeMillis()) == null) {
                        processPendingMutation(nonce, entry.value(), distributor);
                    }
                }
            }
        } catch (KVStoreException e) {
            log.error("Failed to install pending-mutations watch or drain existing entries", e);
        }
    }

    /**
     * Deserializes a {@link ConfigMutation} from the KV entry value, submits it to the
     * {@link ConfigDistributor}, and writes the resulting revision back to the
     * mutation-results prefix so the forwarding follower can complete its future.
     *
     * <p>Errors are logged but do not propagate -- a single bad mutation must not
     * break the watch callback or prevent processing of subsequent mutations.</p>
     *
     * @param nonce       the unique nonce extracted from the KV key
     * @param value       the serialized ConfigMutation bytes
     * @param distributor the distributor to submit the mutation to
     */
    private void processPendingMutation(String nonce, byte[] value, ConfigDistributor distributor) {
        try {
            ConfigMutation mutation = MAPPER.readValue(value, ConfigMutation.class);
            distributor.submit(mutation);

            // The WriteBatcher is asynchronous, but currentRevision() returns the latest
            // committed revision. After submit(), the mutation is enqueued but may not be
            // flushed yet. However, the follower polls with a timeout, and the revision
            // written here tells it "at least this revision". The follower's poll will
            // pick up the result once the batcher flushes and the revision advances.
            //
            // For correctness: we write the current revision. If the batch hasn't flushed
            // yet, this revision may be stale -- but the follower simply sees an older
            // revision and knows the mutation was accepted. The key contract is that the
            // mutation WILL be applied; the exact revision is informational.
            long revision = distributor.currentRevision();
            kvStore.put(MUTATION_RESULTS_PREFIX + nonce, Long.toString(revision).getBytes(StandardCharsets.UTF_8));
            log.debug("Processed pending mutation: nonce={}, revision={}", sanitize(nonce), revision);
        } catch (IOException e) {
            log.error("Failed to deserialize pending mutation: nonce={}", sanitize(nonce), e);
            // Write an error marker so the follower doesn't poll forever.
            // The follower reads the value as Long.parseLong(); a negative value signals error.
            writeErrorResult(nonce);
        } catch (KVStoreException e) {
            log.error("Failed to write mutation result: nonce={}", sanitize(nonce), e);
        } catch (Exception e) {
            log.error("Unexpected error processing pending mutation: nonce={}", sanitize(nonce), e);
            writeErrorResult(nonce);
        }
    }

    /**
     * Best-effort write of an error marker (-1) to the mutation result key so the
     * follower stops polling. The follower will see a negative revision and should
     * treat it as a failure.
     */
    private void writeErrorResult(String nonce) {
        try {
            kvStore.put(MUTATION_RESULTS_PREFIX + nonce, "-1".getBytes(StandardCharsets.UTF_8));
        } catch (KVStoreException e) {
            log.error("Failed to write error result for nonce={}", sanitize(nonce), e);
        }
    }

    /**
     * Closes the pending-mutations watch and cancels the nonce cleanup task
     * if they are currently active.
     */
    private void closePendingMutationsWatch() {
        // Cancel the nonce cleanup task to prevent accumulation under rapid leadership flips
        java.util.concurrent.ScheduledFuture<?> cleanupTask = this.nonceCleanupFuture;
        if (cleanupTask != null) {
            cleanupTask.cancel(false);
            this.nonceCleanupFuture = null;
        }

        Closeable watch = this.pendingMutationsWatch;
        if (watch != null) {
            try {
                watch.close();
                log.info("Closed pending-mutations watch");
            } catch (IOException e) {
                log.warn("Failed to close pending-mutations watch", e);
            }
            this.pendingMutationsWatch = null;
        }
    }

    /**
     * Periodic heartbeat: updates own registration in the KV store with a fresh
     * {@code lastHeartbeat} timestamp. Runs every {@value HEARTBEAT_INTERVAL_SECONDS} seconds.
     */
    private void heartbeat() {
        try {
            ControlPlaneInstance current = peers.get(instanceId);
            if (current == null) {
                log.warn("Self not found in peer map during heartbeat, re-registering");
                registerSelf();
                return;
            }

            ControlPlaneInstance updated = current.withHeartbeat(System.currentTimeMillis());
            byte[] serialized = serializeInstance(updated);
            kvStore.put(instanceKey(instanceId), serialized);
            peers.put(instanceId, updated);
        } catch (Exception e) {
            log.error("Failed to update heartbeat in KV store", e);
        }
    }

    /**
     * Background scanner that removes peers whose {@code lastHeartbeat} is older
     * than {@value STALE_PEER_THRESHOLD_MS} milliseconds. Does NOT delete from the
     * KV store -- the stale instance's own session/TTL mechanism in the KV store
     * will handle that. This only cleans the local in-memory map to avoid routing
     * decisions based on dead peers.
     */
    private void scanStalePeers() {
        try {
            long now = System.currentTimeMillis();
            for (ControlPlaneInstance peer : peers.values()) {
                // Never evict self
                if (peer.instanceId().equals(instanceId)) {
                    continue;
                }

                if (now - peer.lastHeartbeat() > STALE_PEER_THRESHOLD_MS) {
                    ControlPlaneInstance removed = peers.remove(peer.instanceId());
                    if (removed != null) {
                        log.warn("Evicted stale peer: instanceId={}, region={}, lastHeartbeat={}ms ago",
                                removed.instanceId(), removed.region(), now - removed.lastHeartbeat());
                    }
                }
            }
        } catch (Exception e) {
            // Never let an exception kill the scheduled task
            log.error("Error during stale peer scan", e);
        }
    }

    /**
     * Shuts down cluster participation: cancels the watch, withdraws from election,
     * deregisters from the KV store, and stops the scheduler.
     */
    @Override
    public void close() {
        log.info("Shutting down ControlPlaneCluster: instanceId={}", instanceId);

        // Close the pending-mutations watch (leadership processing)
        closePendingMutationsWatch();

        // Cancel the peer watch
        if (instanceWatch != null) {
            try {
                instanceWatch.close();
            } catch (IOException e) {
                log.warn("Failed to close instance watch", e);
            }
        }

        // Withdraw from leader election
        if (leaderElection != null) {
            try {
                leaderElection.close();
            } catch (IOException e) {
                log.warn("Failed to close leader election", e);
            }
        }

        // Deregister self from KV store (best effort)
        try {
            kvStore.delete(instanceKey(instanceId));
        } catch (KVStoreException e) {
            log.warn("Failed to deregister self from KV store", e);
        }

        // Shut down scheduler
        scheduler.shutdown();
        try {
            if (!scheduler.awaitTermination(5, TimeUnit.SECONDS)) {
                scheduler.shutdownNow();
                log.warn("Cluster scheduler did not terminate within 5 seconds, forced shutdown");
            }
        } catch (InterruptedException e) {
            scheduler.shutdownNow();
            Thread.currentThread().interrupt();
        }

        peers.clear();
        isLeader = false;

        log.info("ControlPlaneCluster shut down");
    }

    private static String instanceKey(String instanceId) {
        return INSTANCES_PREFIX + instanceId;
    }

    private static byte[] serializeInstance(ControlPlaneInstance instance) {
        try {
            return MAPPER.writeValueAsBytes(instance);
        } catch (JsonProcessingException e) {
            // ControlPlaneInstance is a simple record with primitives and strings.
            // Serialization failure here is a programming error, not a runtime condition.
            throw new IllegalStateException("Failed to serialize ControlPlaneInstance", e);
        }
    }
}
