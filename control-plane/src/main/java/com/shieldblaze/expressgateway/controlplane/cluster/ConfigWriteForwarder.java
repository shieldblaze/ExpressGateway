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
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;

/**
 * Forwards config mutations from follower CP instances to the cluster leader.
 *
 * <h3>Forwarding mechanism</h3>
 * <p>Rather than opening a direct gRPC channel to the leader (which would require
 * an additional proto service definition and bidirectional CP-to-CP authentication),
 * this component uses the shared KV store as a write-ahead queue:</p>
 *
 * <ol>
 *   <li>The follower serializes the {@link ConfigMutation} and writes it to
 *       {@code /expressgateway/controlplane/pending-mutations/{nonce}}</li>
 *   <li>The leader watches this prefix and processes pending mutations through
 *       the normal {@code ConfigDistributor} pipeline</li>
 *   <li>The result (new revision number) is written back to
 *       {@code /expressgateway/controlplane/mutation-results/{nonce}}</li>
 *   <li>The follower polls for the result and completes the future</li>
 * </ol>
 *
 * <p>This design is simpler and more reliable than direct CP-to-CP gRPC forwarding
 * because it reuses the existing KV store for transport and avoids the leader-identity
 * race condition where a follower might forward to a stale leader.</p>
 *
 * <p>If no leader is currently available (cluster is in election), the mutation is
 * rejected immediately with a {@link KVStoreException}. The caller (REST or gRPC handler)
 * should return an appropriate error to the client.</p>
 *
 * <p>Thread safety: writes are submitted to a dedicated executor to avoid blocking
 * the caller's thread (typically a gRPC/Netty EventLoop).</p>
 */
@Log4j2
public final class ConfigWriteForwarder implements Closeable {

    private static final String PENDING_MUTATIONS_PREFIX = "/expressgateway/controlplane/pending-mutations/";
    private static final String MUTATION_RESULTS_PREFIX = "/expressgateway/controlplane/mutation-results/";
    private static final ObjectMapper MAPPER = new ObjectMapper();

    static {
        MAPPER.findAndRegisterModules();
    }

    /** Maximum time to wait for the leader to process a mutation. */
    private static final long MUTATION_TIMEOUT_MS = 30_000;

    /** Poll interval when waiting for a mutation result. */
    private static final long POLL_INTERVAL_MS = 100;

    private final ControlPlaneCluster cluster;
    private final KVStore kvStore;
    private final ExecutorService executor;

    /**
     * Creates a new config write forwarder.
     *
     * @param cluster the cluster manager providing leadership state; must not be null
     * @param kvStore the KV store used for the pending-mutation queue; must not be null
     */
    public ConfigWriteForwarder(ControlPlaneCluster cluster, KVStore kvStore) {
        this.cluster = Objects.requireNonNull(cluster, "cluster");
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        this.executor = Executors.newFixedThreadPool(4, r -> {
            Thread t = new Thread(r, "cp-write-forwarder");
            t.setDaemon(true);
            return t;
        });
    }

    /**
     * Forwards a config mutation to the leader for processing.
     *
     * <p>If this instance IS the leader, the caller should invoke the local
     * {@code ConfigDistributor} directly instead of calling this method.</p>
     *
     * <p>The returned future completes with the new global config revision assigned
     * by the leader, or completes exceptionally if:</p>
     * <ul>
     *   <li>No leader is available (cluster in election)</li>
     *   <li>The KV store write fails</li>
     *   <li>The leader does not process the mutation within the timeout</li>
     * </ul>
     *
     * @param mutation the config mutation to forward; must not be null
     * @return a future that completes with the new revision from the leader
     * @throws KVStoreException if no leader is available
     */
    public CompletableFuture<Long> forwardMutation(ConfigMutation mutation) throws KVStoreException {
        Objects.requireNonNull(mutation, "mutation");

        if (cluster.isLeader()) {
            throw new IllegalStateException(
                    "This instance is the leader; use ConfigDistributor.submit() directly");
        }

        // Generate a unique nonce for this mutation
        String nonce = cluster.instanceId() + "-" + System.nanoTime();

        CompletableFuture<Long> future = new CompletableFuture<>();

        executor.submit(() -> {
            try {
                // Write the mutation to the pending queue
                byte[] serialized = serializeMutation(mutation);
                kvStore.put(PENDING_MUTATIONS_PREFIX + nonce, serialized);
                log.debug("Submitted pending mutation: nonce={}", nonce);

                // Poll for the result from the leader
                long deadline = System.currentTimeMillis() + MUTATION_TIMEOUT_MS;
                String resultKey = MUTATION_RESULTS_PREFIX + nonce;

                while (System.currentTimeMillis() < deadline) {
                    var result = kvStore.get(resultKey);
                    if (result.isPresent()) {
                        long revision = Long.parseLong(new String(result.get().value()));
                        log.debug("Mutation processed by leader: nonce={}, revision={}", nonce, revision);

                        // Clean up the pending mutation and result entries (best effort)
                        cleanupMutationEntries(nonce);

                        future.complete(revision);
                        return;
                    }

                    //noinspection BusyWait
                    Thread.sleep(POLL_INTERVAL_MS);
                }

                // Timeout -- clean up and fail
                cleanupMutationEntries(nonce);
                future.completeExceptionally(new KVStoreException(
                        KVStoreException.Code.OPERATION_TIMEOUT,
                        "Leader did not process mutation within " + MUTATION_TIMEOUT_MS + "ms, nonce=" + nonce));

            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                future.completeExceptionally(e);
            } catch (Exception e) {
                log.error("Failed to forward mutation: nonce={}", nonce, e);
                future.completeExceptionally(e);
            }
        });

        return future;
    }

    /**
     * Best-effort cleanup of pending and result entries after a mutation is processed or timed out.
     */
    private void cleanupMutationEntries(String nonce) {
        try {
            kvStore.delete(PENDING_MUTATIONS_PREFIX + nonce);
        } catch (KVStoreException e) {
            log.debug("Failed to clean up pending mutation: nonce={}", nonce, e);
        }
        try {
            kvStore.delete(MUTATION_RESULTS_PREFIX + nonce);
        } catch (KVStoreException e) {
            log.debug("Failed to clean up mutation result: nonce={}", nonce, e);
        }
    }

    private static byte[] serializeMutation(ConfigMutation mutation) {
        try {
            return MAPPER.writeValueAsBytes(mutation);
        } catch (JsonProcessingException e) {
            throw new IllegalStateException("Failed to serialize ConfigMutation", e);
        }
    }

    /**
     * Shuts down the forwarding executor. Pending mutations that have not been
     * submitted to the KV store will be lost.
     */
    @Override
    public void close() {
        executor.shutdown();
        try {
            if (!executor.awaitTermination(5, TimeUnit.SECONDS)) {
                executor.shutdownNow();
                log.warn("Write forwarder executor did not terminate within 5 seconds, forced shutdown");
            }
        } catch (InterruptedException e) {
            executor.shutdownNow();
            Thread.currentThread().interrupt();
        }
        log.info("ConfigWriteForwarder shut down");
    }
}
