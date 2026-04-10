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
package com.shieldblaze.expressgateway.coordination.etcd;

import com.shieldblaze.expressgateway.coordination.ConnectionListener;
import com.shieldblaze.expressgateway.coordination.CoordinationEntry;
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import com.shieldblaze.expressgateway.coordination.WatchEvent;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.ClientBuilder;
import io.etcd.jetcd.KV;
import io.etcd.jetcd.KeyValue;
import io.etcd.jetcd.Lease;
import io.etcd.jetcd.Watch;
import io.etcd.jetcd.kv.DeleteResponse;
import io.etcd.jetcd.kv.GetResponse;
import io.etcd.jetcd.kv.PutResponse;
import io.etcd.jetcd.kv.TxnResponse;
import io.etcd.jetcd.lease.LeaseKeepAliveResponse;
import io.etcd.jetcd.op.Cmp;
import io.etcd.jetcd.op.CmpTarget;
import io.etcd.jetcd.op.Op;
import io.etcd.jetcd.options.DeleteOption;
import io.etcd.jetcd.options.GetOption;
import io.etcd.jetcd.options.PutOption;
import io.etcd.jetcd.options.WatchOption;
import io.etcd.jetcd.support.CloseableClient;
import io.etcd.jetcd.watch.WatchResponse;
import io.grpc.StatusRuntimeException;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.io.IOException;
import java.net.ConnectException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;

/**
 * etcd-backed {@link CoordinationProvider} implementation using jetcd 0.8.6.
 *
 * <h2>Key/Version mapping</h2>
 * <p>Keys map directly to etcd key-space paths. The etcd {@code modRevision} field on a
 * {@link KeyValue} is used as the key version. etcd mod revisions are globally monotonic
 * (incremented across all keys in the cluster, not per-key like ZooKeeper). The
 * {@code createRevision} maps to the coordination API's {@code createVersion}.</p>
 *
 * <p>Version 0 has special meaning in CAS: it means "key must not exist" (create-if-absent).
 * etcd naturally uses modRevision=0 for non-existent keys, so this maps cleanly.</p>
 *
 * <h2>Ephemeral keys</h2>
 * <p>Implemented via etcd leases with keep-alive. If this provider closes or the process
 * dies, the lease expires and the key is automatically deleted by etcd.</p>
 *
 * <h2>Sequential keys</h2>
 * <p>etcd does not have native sequential nodes like ZooKeeper. We emulate this with a
 * CAS counter key at {@code {prefix}__seq_counter} and then creating the actual entry at
 * {@code {prefix}{formatted_sequence_number}}.</p>
 *
 * <h2>Thread safety</h2>
 * <p>All operations delegate to the thread-safe jetcd {@link Client}. Watch callbacks and
 * leader election listeners may fire from jetcd internal threads.</p>
 *
 * <h2>Ownership</h2>
 * <p>When constructed via the static factories {@link #create(List)} or
 * {@link #create(List, String, String)}, this provider owns the Client and will close it
 * on {@link #close()}. When constructed with an externally-provided Client, ownership
 * semantics are controlled by the {@code ownsClient} parameter.</p>
 */
@Log4j2
public final class EtcdCoordinationProvider implements CoordinationProvider {

    /** Default timeout for individual etcd operations. */
    private static final long OP_TIMEOUT_SECONDS = 10;

    /** TTL for ephemeral key leases. */
    private static final long EPHEMERAL_LEASE_TTL_SECONDS = 30;

    /** Width of zero-padded sequential key suffix. */
    private static final int SEQ_PAD_WIDTH = 10;

    private final Client client;
    private final boolean ownsClient;
    private final CopyOnWriteArrayList<ConnectionListener> connectionListeners;
    private final CopyOnWriteArrayList<CloseableClient> keepAliveHandles;
    private final AtomicBoolean closed;

    /**
     * Creates a provider wrapping an externally-managed jetcd Client.
     * The caller retains ownership of the client and must close it separately.
     *
     * @param client an already-built jetcd Client instance
     */
    public EtcdCoordinationProvider(Client client) {
        this(client, false);
    }

    /**
     * Constructor controlling ownership semantics.
     *
     * @param client     the jetcd Client instance
     * @param ownsClient if true, close() will also close the Client
     */
    EtcdCoordinationProvider(Client client, boolean ownsClient) {
        this.client = Objects.requireNonNull(client, "client");
        this.ownsClient = ownsClient;
        this.connectionListeners = new CopyOnWriteArrayList<>();
        this.keepAliveHandles = new CopyOnWriteArrayList<>();
        this.closed = new AtomicBoolean(false);
    }

    /**
     * Convenience factory that creates a new jetcd Client from endpoints.
     * The returned provider owns the client and will close it on {@link #close()}.
     *
     * @param endpoints list of etcd endpoints (e.g. "http://localhost:2379")
     * @return a fully initialized provider
     */
    public static EtcdCoordinationProvider create(List<String> endpoints) {
        Client client = Client.builder()
                .endpoints(endpoints.toArray(new String[0]))
                .build();
        return new EtcdCoordinationProvider(client, true);
    }

    /**
     * Convenience factory with authentication credentials.
     *
     * @param endpoints list of etcd endpoints
     * @param username  the username for authentication
     * @param password  the password for authentication
     * @return a fully initialized provider
     */
    public static EtcdCoordinationProvider create(List<String> endpoints,
                                                   String username,
                                                   String password) {
        ClientBuilder builder = Client.builder()
                .endpoints(endpoints.toArray(new String[0]));

        if (username != null && !username.isBlank()) {
            builder.user(bs(username));
        }
        if (password != null && !password.isBlank()) {
            builder.password(bs(password));
        }

        Client client = builder.build();
        return new EtcdCoordinationProvider(client, true);
    }

    // ---- Key-Value CRUD ----

    @Override
    public Optional<CoordinationEntry> get(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        try {
            GetResponse response = client.getKVClient()
                    .get(bs(key))
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            if (response.getKvs().isEmpty()) {
                return Optional.empty();
            }

            KeyValue kv = response.getKvs().getFirst();
            return Optional.of(toEntry(kv));
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while getting key: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout getting key: " + key, e);
        }
    }

    @Override
    public long put(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            PutResponse response = client.getKVClient()
                    .put(bs(key), ByteSequence.from(value))
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            // The header revision is the etcd cluster revision after this put.
            // We need the modRevision of the key itself, which equals the header revision
            // for the key that was just written.
            return response.getHeader().getRevision();
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while putting key: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout putting key: " + key, e);
        }
    }

    @Override
    public long cas(String key, byte[] value, long expectedVersion) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        if (expectedVersion < 0) {
            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "expectedVersion must be >= 0, got: " + expectedVersion);
        }

        try {
            KV kvClient = client.getKVClient();
            ByteSequence bsKey = bs(key);
            ByteSequence bsValue = ByteSequence.from(value);

            TxnResponse txnResponse;

            if (expectedVersion == 0) {
                // Create-if-absent: key must not exist.
                // In etcd, a non-existent key has version == 0 (the per-key version counter,
                // NOT modRevision). We use CmpTarget.version(0) which checks the per-key
                // version field.
                txnResponse = kvClient.txn()
                        .If(new Cmp(bsKey, Cmp.Op.EQUAL, CmpTarget.version(0)))
                        .Then(Op.put(bsKey, bsValue, PutOption.DEFAULT))
                        .Else(Op.get(bsKey, GetOption.DEFAULT))
                        .commit()
                        .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

                if (!txnResponse.isSucceeded()) {
                    throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                            "Key already exists (CAS with expectedVersion=0): " + key);
                }
                return txnResponse.getHeader().getRevision();

            } else {
                // CAS update: the key's modRevision must equal expectedVersion.
                txnResponse = kvClient.txn()
                        .If(new Cmp(bsKey, Cmp.Op.EQUAL, CmpTarget.modRevision(expectedVersion)))
                        .Then(Op.put(bsKey, bsValue, PutOption.DEFAULT))
                        .Else(Op.get(bsKey, GetOption.DEFAULT))
                        .commit()
                        .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

                if (!txnResponse.isSucceeded()) {
                    // Determine whether the key doesn't exist or version mismatched
                    List<GetResponse> elseGets = txnResponse.getGetResponses();
                    if (!elseGets.isEmpty() && elseGets.getFirst().getKvs().isEmpty()) {
                        throw new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                                "Key not found for CAS update: " + key);
                    }
                    throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                            "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion);
                }
                return txnResponse.getHeader().getRevision();
            }
        } catch (CoordinationException e) {
            throw e;
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted during CAS on key: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout during CAS on key: " + key, e);
        }
    }

    @Override
    public boolean delete(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        try {
            DeleteResponse response = client.getKVClient()
                    .delete(bs(key))
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);
            return response.getDeleted() > 0;
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while deleting key: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout deleting key: " + key, e);
        }
    }

    @Override
    public void deleteTree(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        String prefix = normalizePrefix(key);
        try {
            // Delete the prefix key itself
            client.getKVClient()
                    .delete(bs(key))
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            // Delete all keys with this prefix (children)
            client.getKVClient()
                    .delete(bs(prefix), DeleteOption.builder().isPrefix(true).build())
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while deleting tree: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout deleting tree: " + key, e);
        }
    }

    @Override
    public List<CoordinationEntry> list(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePrefix(prefix);
        try {
            GetResponse response = client.getKVClient()
                    .get(bs(normalizedPrefix), GetOption.builder()
                            .isPrefix(true)
                            .withKeysOnly(false)
                            .build())
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            if (response.getKvs().isEmpty()) {
                return Collections.emptyList();
            }

            // Filter to direct children only: keys that have exactly one additional
            // path segment after the prefix. For prefix "/a/b/", a direct child is
            // "/a/b/c" but NOT "/a/b/c/d".
            List<CoordinationEntry> entries = new ArrayList<>();
            for (KeyValue kv : response.getKvs()) {
                String fullKey = str(kv.getKey());
                String remainder = fullKey.substring(normalizedPrefix.length());

                // Direct child: no '/' in remainder (or empty remainder means the prefix itself)
                if (!remainder.isEmpty() && !remainder.contains("/")) {
                    entries.add(toEntry(kv));
                }
            }
            return entries;
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), prefix);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while listing prefix: " + prefix, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout listing prefix: " + prefix, e);
        }
    }

    @Override
    public List<CoordinationEntry> listRecursive(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePrefix(prefix);
        try {
            GetResponse response = client.getKVClient()
                    .get(bs(normalizedPrefix), GetOption.builder()
                            .isPrefix(true)
                            .build())
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            if (response.getKvs().isEmpty()) {
                return Collections.emptyList();
            }

            List<CoordinationEntry> entries = new ArrayList<>(response.getKvs().size());
            for (KeyValue kv : response.getKvs()) {
                String fullKey = str(kv.getKey());
                // Exclude the prefix key itself if it matches exactly
                String remainder = fullKey.substring(normalizedPrefix.length());
                if (!remainder.isEmpty()) {
                    entries.add(toEntry(kv));
                }
            }
            return entries;
        } catch (ExecutionException e) {
            throw mapException(e.getCause(), prefix);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while listing recursive prefix: " + prefix, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout listing recursive prefix: " + prefix, e);
        }
    }

    // ---- Ephemeral and Sequential keys ----

    @Override
    public long putEphemeral(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            Lease leaseClient = client.getLeaseClient();

            // Create a lease with TTL
            long leaseId = leaseClient.grant(EPHEMERAL_LEASE_TTL_SECONDS)
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                    .getID();

            // Start keep-alive to prevent the lease from expiring while this provider is alive
            CloseableClient keepAlive = leaseClient.keepAlive(leaseId,
                    new io.grpc.stub.StreamObserver<LeaseKeepAliveResponse>() {
                        @Override
                        public void onNext(LeaseKeepAliveResponse response) {
                            log.trace("Lease {} keep-alive renewed, TTL={}", leaseId, response.getTTL());
                        }

                        @Override
                        public void onError(Throwable t) {
                            log.warn("Lease {} keep-alive error for ephemeral key {}", leaseId, key, t);
                            notifyConnectionListeners(ConnectionListener.ConnectionState.SUSPENDED);
                        }

                        @Override
                        public void onCompleted() {
                            log.debug("Lease {} keep-alive completed for ephemeral key {}", leaseId, key);
                        }
                    });

            // Track the keep-alive handle for cleanup on close()
            keepAliveHandles.add(keepAlive);

            // Put the key with the lease
            PutOption putOption = PutOption.builder().withLeaseId(leaseId).build();
            PutResponse response = client.getKVClient()
                    .put(bs(key), ByteSequence.from(value), putOption)
                    .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

            log.debug("Created ephemeral key {} with lease {} (TTL={}s)",
                    key, leaseId, EPHEMERAL_LEASE_TTL_SECONDS);

            return response.getHeader().getRevision();

        } catch (ExecutionException e) {
            throw mapException(e.getCause(), key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Interrupted while putting ephemeral key: " + key, e);
        } catch (TimeoutException e) {
            throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Timeout putting ephemeral key: " + key, e);
        }
    }

    @Override
    public String putSequential(String keyPrefix, byte[] value) throws CoordinationException {
        Objects.requireNonNull(keyPrefix, "keyPrefix");
        Objects.requireNonNull(value, "value");

        // etcd does not have native sequential nodes. We emulate using a CAS counter.
        // The counter key stores the current sequence number; we CAS-increment it,
        // then create the actual entry with the sequence number as suffix.
        String counterKey = keyPrefix + "__seq_counter";

        int maxRetries = 20;
        for (int attempt = 0; attempt < maxRetries; attempt++) {
            try {
                KV kvClient = client.getKVClient();

                // Read the current counter value
                GetResponse counterGet = kvClient.get(bs(counterKey))
                        .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

                long currentSeq;
                long currentModRevision;

                if (counterGet.getKvs().isEmpty()) {
                    // Counter does not exist yet -- first sequence
                    currentSeq = 0;
                    currentModRevision = 0;
                } else {
                    KeyValue counterKv = counterGet.getKvs().getFirst();
                    currentSeq = Long.parseLong(str(counterKv.getValue()));
                    currentModRevision = counterKv.getModRevision();
                }

                long nextSeq = currentSeq + 1;
                ByteSequence newCounterValue = bs(String.valueOf(nextSeq));
                ByteSequence counterKeyBs = bs(counterKey);

                // CAS-increment the counter
                TxnResponse txnResponse;
                if (currentModRevision == 0) {
                    // Counter does not exist: create it
                    txnResponse = kvClient.txn()
                            .If(new Cmp(counterKeyBs, Cmp.Op.EQUAL, CmpTarget.version(0)))
                            .Then(Op.put(counterKeyBs, newCounterValue, PutOption.DEFAULT))
                            .commit()
                            .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);
                } else {
                    // Counter exists: CAS on modRevision
                    txnResponse = kvClient.txn()
                            .If(new Cmp(counterKeyBs, Cmp.Op.EQUAL, CmpTarget.modRevision(currentModRevision)))
                            .Then(Op.put(counterKeyBs, newCounterValue, PutOption.DEFAULT))
                            .commit()
                            .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);
                }

                if (!txnResponse.isSucceeded()) {
                    // CAS failed -- another writer incremented the counter; retry
                    log.debug("Sequential counter CAS conflict on {}, retrying (attempt {})", counterKey, attempt);
                    continue;
                }

                // Counter incremented successfully -- create the actual sequential key
                String formattedSeq = String.format("%0" + SEQ_PAD_WIDTH + "d", nextSeq);
                String fullKey = keyPrefix + formattedSeq;

                kvClient.put(bs(fullKey), ByteSequence.from(value))
                        .get(OP_TIMEOUT_SECONDS, TimeUnit.SECONDS);

                log.debug("Created sequential key {}", fullKey);
                return fullKey;

            } catch (ExecutionException e) {
                throw mapException(e.getCause(), keyPrefix);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Interrupted while creating sequential key: " + keyPrefix, e);
            } catch (TimeoutException e) {
                throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Timeout creating sequential key: " + keyPrefix, e);
            }
        }

        throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                "Failed to create sequential key after " + maxRetries
                        + " attempts due to contention: " + keyPrefix);
    }

    // ---- Watch ----

    @Override
    public Closeable watch(String keyOrPrefix, Consumer<WatchEvent> listener) throws CoordinationException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(listener, "listener");

        try {
            WatchOption watchOption = WatchOption.builder()
                    .isPrefix(true)
                    .withPrevKV(true)
                    .build();

            Watch.Watcher watcher = client.getWatchClient().watch(
                    bs(keyOrPrefix),
                    watchOption,
                    Watch.listener(
                            watchResponse -> {
                                for (io.etcd.jetcd.watch.WatchEvent event : watchResponse.getEvents()) {
                                    try {
                                        WatchEvent mapped = mapWatchEvent(event);
                                        listener.accept(mapped);
                                    } catch (Exception e) {
                                        log.error("Error dispatching watch event for prefix {}",
                                                keyOrPrefix, e);
                                    }
                                }
                            },
                            throwable -> {
                                log.error("Watch error on {}", keyOrPrefix, throwable);
                                notifyConnectionListeners(ConnectionListener.ConnectionState.SUSPENDED);
                            },
                            () -> log.debug("Watch completed on {}", keyOrPrefix)
                    )
            );

            log.debug("Installed watch on {}", keyOrPrefix);

            return () -> {
                log.debug("Closing watch on {}", keyOrPrefix);
                watcher.close();
            };

        } catch (Exception e) {
            throw mapException(e, keyOrPrefix);
        }
    }

    // ---- Leader election ----

    @Override
    public LeaderElection leaderElection(String path, String participantId) throws CoordinationException {
        Objects.requireNonNull(path, "path");
        Objects.requireNonNull(participantId, "participantId");
        return new EtcdLeaderElection(client, path, participantId);
    }

    // ---- Distributed locking ----

    @Override
    public DistributedLock acquireLock(String lockPath, long timeout, TimeUnit unit)
            throws CoordinationException {
        Objects.requireNonNull(lockPath, "lockPath");
        Objects.requireNonNull(unit, "unit");
        return EtcdDistributedLock.acquire(client, lockPath, timeout, unit);
    }

    // ---- Batch operations (optimized with etcd transactions) ----

    @Override
    public void batchPut(Map<String, byte[]> entries) throws CoordinationException {
        Objects.requireNonNull(entries, "entries");
        if (entries.isEmpty()) {
            return;
        }

        // etcd transactions have a limit on the number of operations (typically 128).
        // For small batches, use a single transaction. For larger batches, fall back
        // to sequential puts.
        if (entries.size() <= 128) {
            try {
                Op[] ops = entries.entrySet().stream()
                        .map(e -> Op.put(bs(e.getKey()), ByteSequence.from(e.getValue()), PutOption.DEFAULT))
                        .toArray(Op[]::new);

                client.getKVClient().txn()
                        .Then(ops)
                        .commit()
                        .get(OP_TIMEOUT_SECONDS * 2, TimeUnit.SECONDS);
            } catch (ExecutionException e) {
                throw mapException(e.getCause(), "batchPut");
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Interrupted during batch put", e);
            } catch (TimeoutException e) {
                throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Timeout during batch put", e);
            }
        } else {
            // Fall back to default sequential implementation for large batches
            CoordinationProvider.super.batchPut(entries);
        }
    }

    // ---- Connection health ----

    @Override
    public boolean isConnected() {
        if (closed.get()) {
            return false;
        }
        try {
            // Lightweight health check: attempt to get a non-existent key with short timeout.
            // If the cluster is reachable, this returns quickly with an empty result.
            client.getKVClient()
                    .get(bs("/___health_check___"))
                    .get(3, TimeUnit.SECONDS);
            return true;
        } catch (Exception e) {
            log.debug("Health check failed", e);
            return false;
        }
    }

    @Override
    public void addConnectionListener(ConnectionListener listener) {
        Objects.requireNonNull(listener, "listener");
        connectionListeners.add(listener);
    }

    // ---- Lifecycle ----

    @Override
    public void close() throws IOException {
        if (!closed.compareAndSet(false, true)) {
            return;
        }
        log.debug("Closing EtcdCoordinationProvider (ownsClient={})", ownsClient);

        // Close all keep-alive handles
        for (CloseableClient handle : keepAliveHandles) {
            try {
                handle.close();
            } catch (Exception e) {
                log.warn("Error closing keep-alive handle", e);
            }
        }
        keepAliveHandles.clear();

        if (ownsClient) {
            client.close();
        }

        notifyConnectionListeners(ConnectionListener.ConnectionState.LOST);
    }

    // ---- Internal helpers ----

    /**
     * Converts a jetcd {@link KeyValue} to a {@link CoordinationEntry}.
     *
     * <p>Version mapping: etcd's {@code modRevision} (globally monotonic) is used as the
     * entry version. etcd's {@code createRevision} is used as the entry createVersion.</p>
     */
    private static CoordinationEntry toEntry(KeyValue kv) {
        return new CoordinationEntry(
                str(kv.getKey()),
                kv.getValue().getBytes(),
                kv.getModRevision(),
                kv.getCreateRevision()
        );
    }

    /**
     * Maps a jetcd watch event to the coordination API's {@link WatchEvent}.
     */
    private static WatchEvent mapWatchEvent(io.etcd.jetcd.watch.WatchEvent event) {
        WatchEvent.Type type = switch (event.getEventType()) {
            case PUT -> WatchEvent.Type.PUT;
            case DELETE -> WatchEvent.Type.DELETE;
            case UNRECOGNIZED -> WatchEvent.Type.PUT; // Defensive: treat unknown as PUT
        };

        CoordinationEntry entry = null;
        CoordinationEntry previousEntry = null;

        if (type == WatchEvent.Type.PUT) {
            entry = toEntry(event.getKeyValue());
            if (event.getPrevKV() != null && event.getPrevKV().getModRevision() > 0) {
                previousEntry = toEntry(event.getPrevKV());
            }
        } else if (type == WatchEvent.Type.DELETE) {
            // For DELETE events, entry is null and previousEntry holds the deleted key's last state
            if (event.getPrevKV() != null && event.getPrevKV().getModRevision() > 0) {
                previousEntry = toEntry(event.getPrevKV());
            }
        }

        return new WatchEvent(type, entry, previousEntry);
    }

    /**
     * Ensures a prefix ends with '/' for correct prefix-based queries.
     * etcd prefix queries include all keys starting with the given bytes, so
     * we normalize to ensure "/a/b/" matches "/a/b/c" but not "/a/bc".
     */
    private static String normalizePrefix(String prefix) {
        if (prefix.endsWith("/")) {
            return prefix;
        }
        return prefix + "/";
    }

    /**
     * Maps etcd/gRPC exceptions to {@link CoordinationException} with appropriate codes.
     * Package-private so other etcd classes can use it.
     */
    static CoordinationException mapException(Throwable e, String key) {
        if (e instanceof CoordinationException ce) {
            return ce;
        }

        // Unwrap CompletionException / ExecutionException
        if (e instanceof java.util.concurrent.CompletionException && e.getCause() != null) {
            return mapException(e.getCause(), key);
        }
        if (e instanceof ExecutionException && e.getCause() != null) {
            return mapException(e.getCause(), key);
        }

        // gRPC status-based errors
        if (e instanceof StatusRuntimeException sre) {
            return switch (sre.getStatus().getCode()) {
                case UNAVAILABLE, DEADLINE_EXCEEDED ->
                        new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                                "Connection lost while accessing key: " + key, e);
                case NOT_FOUND ->
                        new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                                "Key not found: " + key, e);
                case ALREADY_EXISTS ->
                        new CoordinationException(CoordinationException.Code.KEY_EXISTS,
                                "Key already exists: " + key, e);
                case PERMISSION_DENIED, UNAUTHENTICATED ->
                        new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                                "Authentication/authorization error for key: " + key, e);
                default ->
                        new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                                "gRPC error (" + sre.getStatus().getCode() + ") for key: " + key, e);
            };
        }

        return switch (e) {
            case ConnectException _ ->
                    new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                            "Connection refused for key: " + key, e);
            case InterruptedException _ -> {
                Thread.currentThread().interrupt();
                yield new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                        "Operation interrupted for key: " + key, e);
            }
            case TimeoutException _ ->
                    new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                            "Operation timed out for key: " + key, e);
            default -> new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Internal error for key: " + key, e);
        };
    }

    /**
     * Notifies all registered connection listeners of a state change.
     */
    private void notifyConnectionListeners(ConnectionListener.ConnectionState state) {
        for (ConnectionListener listener : connectionListeners) {
            try {
                listener.onConnectionStateChange(state);
            } catch (Exception e) {
                log.error("Error notifying connection listener of state {}", state, e);
            }
        }
    }

    /**
     * Converts a String to a ByteSequence using UTF-8.
     */
    private static ByteSequence bs(String s) {
        return ByteSequence.from(s, StandardCharsets.UTF_8);
    }

    /**
     * Converts a ByteSequence to a String using UTF-8.
     */
    private static String str(ByteSequence bs) {
        return bs.toString(StandardCharsets.UTF_8);
    }
}
