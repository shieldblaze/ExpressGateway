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
package com.shieldblaze.expressgateway.controlplane.kvstore.etcd;

import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatchEvent;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import io.etcd.jetcd.ByteSequence;
import io.etcd.jetcd.Client;
import io.etcd.jetcd.Election;
import io.etcd.jetcd.KeyValue;
import io.etcd.jetcd.Watch;
import io.etcd.jetcd.election.CampaignResponse;
import io.etcd.jetcd.election.LeaderKey;
import io.etcd.jetcd.election.LeaderResponse;
import io.etcd.jetcd.common.exception.CompactedException;
import io.etcd.jetcd.common.exception.ErrorCode;
import io.etcd.jetcd.common.exception.EtcdException;
import io.etcd.jetcd.kv.DeleteResponse;
import io.etcd.jetcd.kv.GetResponse;
import io.etcd.jetcd.kv.PutResponse;
import io.etcd.jetcd.kv.TxnResponse;
import io.etcd.jetcd.lease.LeaseGrantResponse;
import io.etcd.jetcd.lock.LockResponse;
import io.etcd.jetcd.op.Cmp;
import io.etcd.jetcd.op.CmpTarget;
import io.etcd.jetcd.op.Op;
import io.etcd.jetcd.options.DeleteOption;
import io.etcd.jetcd.options.GetOption;
import io.etcd.jetcd.options.PutOption;
import io.etcd.jetcd.options.WatchOption;
import io.etcd.jetcd.support.CloseableClient;
import io.etcd.jetcd.watch.WatchEvent;
import io.etcd.jetcd.watch.WatchResponse;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.ExecutionException;

/**
 * etcd-backed {@link KVStore} implementation using jetcd (io.etcd:jetcd-core:0.8.6).
 *
 * <p>Key mapping: KV keys map directly to etcd key paths. The etcd key's
 * {@code modRevision} is used as the KV version (globally monotonically increasing
 * on each mutation in the etcd cluster). {@code createRevision} maps to the KV
 * createVersion.</p>
 *
 * <p><strong>Important difference from ZooKeeper:</strong> etcd versions (revisions)
 * are global and monotonically increasing across all keys, not per-key counters.
 * The {@code modRevision} returned from {@link #put} and {@link #cas} is the
 * cluster-wide revision at which the write occurred.</p>
 *
 * <p>Thread safety: all operations delegate to the thread-safe jetcd {@link Client}.
 * Watch callbacks, leader election listeners, and keep-alive operations may fire
 * from jetcd's internal gRPC threads.</p>
 */
@Log4j2
public final class EtcdKVStore implements KVStore {

    private static final long LOCK_LEASE_TTL_SECONDS = 30;
    private static final long ELECTION_LEASE_TTL_SECONDS = 15;

    private final Client client;

    /**
     * Creates a new EtcdKVStore backed by the given jetcd client.
     *
     * @param client The jetcd client instance (lifecycle managed externally)
     * @throws NullPointerException if client is null
     */
    public EtcdKVStore(Client client) {
        this.client = Objects.requireNonNull(client, "client");
    }

    @Override
    public Optional<KVEntry> get(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            GetResponse response = client.getKVClient()
                    .get(toByteSequence(key))
                    .get();
            if (response.getCount() == 0) {
                return Optional.empty();
            }
            KeyValue kv = response.getKvs().get(0);
            return Optional.of(toEntry(kv));
        } catch (ExecutionException e) {
            throw mapException(e, key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
    }

    @Override
    public List<KVEntry> list(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        try {
            // Ensure the prefix ends with '/' for consistent direct-children filtering.
            String normalizedPrefix = prefix.endsWith("/") ? prefix : prefix + "/";

            GetOption option = GetOption.builder()
                    .isPrefix(true)
                    .build();

            GetResponse response = client.getKVClient()
                    .get(toByteSequence(normalizedPrefix), option)
                    .get();

            if (response.getCount() == 0) {
                return Collections.emptyList();
            }

            // Filter to direct children only: keys that are exactly one path segment
            // deeper than the prefix (i.e., no additional '/' after the prefix).
            List<KVEntry> entries = new ArrayList<>();
            for (KeyValue kv : response.getKvs()) {
                String fullKey = kv.getKey().toString(StandardCharsets.UTF_8);
                String remainder = fullKey.substring(normalizedPrefix.length());
                // Direct child: remainder is non-empty and contains no '/'
                if (!remainder.isEmpty() && remainder.indexOf('/') == -1) {
                    entries.add(toEntry(kv));
                }
            }
            return entries;
        } catch (ExecutionException e) {
            throw mapException(e, prefix);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for prefix: " + prefix, e);
        }
    }

    @Override
    public List<KVEntry> listRecursive(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        try {
            // Ensure the prefix ends with '/' for consistent hierarchical listing.
            String normalizedPrefix = prefix.endsWith("/") ? prefix : prefix + "/";

            GetOption option = GetOption.builder()
                    .isPrefix(true)
                    .build();

            GetResponse response = client.getKVClient()
                    .get(toByteSequence(normalizedPrefix), option)
                    .get();

            if (response.getCount() == 0) {
                return Collections.emptyList();
            }

            // Return all descendants -- etcd prefix-based listing already returns all keys
            // under the prefix at any depth.
            List<KVEntry> entries = new ArrayList<>((int) response.getCount());
            for (KeyValue kv : response.getKvs()) {
                entries.add(toEntry(kv));
            }
            return entries;
        } catch (ExecutionException e) {
            throw mapException(e, prefix);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for prefix: " + prefix, e);
        }
    }

    @Override
    public long put(String key, byte[] value) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            PutOption option = PutOption.builder()
                    .withPrevKV()
                    .build();

            PutResponse response = client.getKVClient()
                    .put(toByteSequence(key), ByteSequence.from(value), option)
                    .get();

            // The header revision is the cluster-wide revision at which this put occurred,
            // which equals the new modRevision of the key.
            return response.getHeader().getRevision();
        } catch (ExecutionException e) {
            throw mapException(e, key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
    }

    @Override
    public long cas(String key, byte[] value, long expectedVersion) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        if (expectedVersion < 0) {
            throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                    "expectedVersion must be >= 0, got: " + expectedVersion);
        }
        try {
            ByteSequence keyBs = toByteSequence(key);
            ByteSequence valueBs = ByteSequence.from(value);

            Cmp condition;
            if (expectedVersion == 0) {
                // Create-if-absent: key must not exist (createRevision == 0 means non-existent)
                condition = new Cmp(keyBs, Cmp.Op.EQUAL, CmpTarget.createRevision(0));
            } else {
                // CAS update: current modRevision must match expectedVersion
                condition = new Cmp(keyBs, Cmp.Op.EQUAL, CmpTarget.modRevision(expectedVersion));
            }

            TxnResponse txnResponse = client.getKVClient().txn()
                    .If(condition)
                    .Then(Op.put(keyBs, valueBs, PutOption.DEFAULT))
                    .Else(Op.get(keyBs, GetOption.DEFAULT))
                    .commit()
                    .get();

            if (txnResponse.isSucceeded()) {
                // The header revision is the revision at which the transaction committed,
                // which is the new modRevision of the key.
                return txnResponse.getHeader().getRevision();
            }

            // Transaction failed: version mismatch
            if (expectedVersion == 0) {
                throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                        "Key already exists (CAS with expectedVersion=0): " + key);
            } else {
                throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                        "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion);
            }
        } catch (KVStoreException e) {
            throw e;
        } catch (ExecutionException e) {
            throw mapException(e, key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
    }

    @Override
    public boolean delete(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            DeleteResponse response = client.getKVClient()
                    .delete(toByteSequence(key))
                    .get();
            return response.getDeleted() > 0;
        } catch (ExecutionException e) {
            throw mapException(e, key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
    }

    @Override
    public void deleteTree(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            // Delete the exact key itself
            client.getKVClient()
                    .delete(toByteSequence(key))
                    .get();

            // Delete all children under key + "/" to avoid deleting siblings
            // (e.g., deleting "/foo" should not delete "/foobar", only "/foo/...")
            String childPrefix = key.endsWith("/") ? key : key + "/";
            DeleteOption option = DeleteOption.builder()
                    .isPrefix(true)
                    .build();

            client.getKVClient()
                    .delete(toByteSequence(childPrefix), option)
                    .get();
        } catch (ExecutionException e) {
            throw mapException(e, key);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
    }

    @Override
    public Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(watcher, "watcher");

        CompactionAwareWatchHolder holder = new CompactionAwareWatchHolder(
                client, keyOrPrefix, watcher);
        holder.startWatch(0);

        log.debug("Installed watch on {}", keyOrPrefix);

        return () -> {
            log.debug("Closing watch on {}", keyOrPrefix);
            holder.close();
        };
    }

    /**
     * Manages an etcd watch that automatically recovers from compaction events.
     *
     * <p>When etcd compacts its history, watches on old revisions receive a
     * {@link CompactedException}. This holder detects the error, replays missed
     * events by doing a full prefix list (synthetic PUT events), and re-creates
     * the watch from the current revision.</p>
     */
    private static final class CompactionAwareWatchHolder {

        private final Client client;
        private final String keyOrPrefix;
        private final KVWatcher watcher;
        private volatile Watch.Watcher currentWatcher;
        private volatile boolean closed;

        CompactionAwareWatchHolder(Client client, String keyOrPrefix, KVWatcher watcher) {
            this.client = client;
            this.keyOrPrefix = keyOrPrefix;
            this.watcher = watcher;
        }

        /**
         * Starts (or restarts) the watch from the given revision.
         * If {@code fromRevision} is 0, watches from the latest revision.
         */
        void startWatch(long fromRevision) {
            if (closed) {
                return;
            }

            WatchOption.Builder optionBuilder = WatchOption.builder()
                    .isPrefix(true)
                    .withPrevKV(true);

            if (fromRevision > 0) {
                optionBuilder.withRevision(fromRevision);
            }

            WatchOption option = optionBuilder.build();

            currentWatcher = client.getWatchClient().watch(
                    toByteSequence(keyOrPrefix),
                    option,
                    Watch.listener(
                            watchResponse -> dispatchWatchEvents(watchResponse, watcher, keyOrPrefix),
                            this::handleWatchError
                    )
            );
        }

        private void handleWatchError(Throwable throwable) {
            if (closed) {
                return;
            }

            if (isCompactionError(throwable)) {
                log.warn("Watch compaction detected for prefix '{}'. " +
                        "Replaying current state and re-establishing watch.", keyOrPrefix);
                recoverFromCompaction();
            } else {
                log.error("Watch error for {}", keyOrPrefix, throwable);
            }
        }

        /**
         * Checks whether the throwable indicates an etcd compaction event.
         */
        private static boolean isCompactionError(Throwable throwable) {
            if (throwable instanceof CompactedException _) {
                return true;
            }
            // Walk the cause chain -- jetcd sometimes wraps CompactedException
            Throwable cause = throwable;
            while (cause != null) {
                if (cause instanceof CompactedException _) {
                    return true;
                }
                // Also check the message for gRPC status detail matching compaction
                String message = cause.getMessage();
                if (message != null && message.contains("compacted")) {
                    return true;
                }
                cause = cause.getCause();
            }
            return false;
        }

        /**
         * Recovers from a compaction event by:
         * 1. Closing the stale watcher
         * 2. Performing a full prefix listing to get current state
         * 3. Replaying all entries as synthetic PUT events
         * 4. Re-establishing the watch from the current revision
         */
        private void recoverFromCompaction() {
            // Close the stale watcher
            Watch.Watcher stale = currentWatcher;
            if (stale != null) {
                stale.close();
            }

            if (closed) {
                return;
            }

            try {
                // Full prefix list to replay missed events
                GetOption option = GetOption.builder()
                        .isPrefix(true)
                        .build();

                GetResponse response = client.getKVClient()
                        .get(toByteSequence(keyOrPrefix), option)
                        .get();

                // Determine the revision to watch from (after the current state)
                long replayRevision = response.getHeader().getRevision();

                // Replay all existing keys as PUT events so the watcher's state is consistent
                for (KeyValue kv : response.getKvs()) {
                    try {
                        KVEntry entry = toEntry(kv);
                        watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.PUT, entry, null));
                    } catch (Exception e) {
                        log.error("Error replaying watch event during compaction recovery for {}",
                                keyOrPrefix, e);
                    }
                }

                // Re-establish watch from the revision after the snapshot
                startWatch(replayRevision + 1);

                log.info("Watch recovered from compaction for prefix '{}'. " +
                        "Replayed {} entries, resuming from revision {}",
                        keyOrPrefix, response.getCount(), replayRevision + 1);

            } catch (Exception e) {
                log.error("Failed to recover watch from compaction for prefix '{}'. " +
                        "Watch is no longer active.", keyOrPrefix, e);
            }
        }

        void close() {
            closed = true;
            Watch.Watcher w = currentWatcher;
            if (w != null) {
                w.close();
            }
        }
    }

    @Override
    public Closeable acquireLock(String lockPath) throws KVStoreException {
        Objects.requireNonNull(lockPath, "lockPath");
        try {
            // Grant a lease for the lock
            LeaseGrantResponse leaseGrant = client.getLeaseClient()
                    .grant(LOCK_LEASE_TTL_SECONDS)
                    .get();
            long leaseId = leaseGrant.getID();

            // Start keep-alive to prevent lease expiration while lock is held
            CloseableClient keepAlive = client.getLeaseClient()
                    .keepAlive(leaseId, new NoOpStreamObserver<>());

            // Acquire the lock (blocks until acquired)
            LockResponse lockResponse;
            try {
                lockResponse = client.getLockClient()
                        .lock(toByteSequence(lockPath), leaseId)
                        .get();
            } catch (Exception e) {
                // Clean up lease resources if lock acquisition fails
                keepAlive.close();
                silentRevokeLease(leaseId);
                throw e;
            }

            ByteSequence lockKey = lockResponse.getKey();
            log.debug("Acquired lock on {} with lease {}", lockPath, leaseId);

            return () -> {
                try {
                    client.getLockClient().unlock(lockKey).get();
                    log.debug("Released lock on {}", lockPath);
                } catch (Exception e) {
                    log.error("Error releasing lock on {}", lockPath, e);
                } finally {
                    keepAlive.close();
                    silentRevokeLease(leaseId);
                }
            };
        } catch (ExecutionException e) {
            throw mapException(e, lockPath);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted while acquiring lock: " + lockPath, e);
        }
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException {
        Objects.requireNonNull(electionPath, "electionPath");
        Objects.requireNonNull(participantId, "participantId");
        try {
            // Grant a lease for the election campaign
            LeaseGrantResponse leaseGrant = client.getLeaseClient()
                    .grant(ELECTION_LEASE_TTL_SECONDS)
                    .get();
            long leaseId = leaseGrant.getID();

            // Start keep-alive to maintain the lease
            CloseableClient keepAlive = client.getLeaseClient()
                    .keepAlive(leaseId, new NoOpStreamObserver<>());

            EtcdLeaderElection election = new EtcdLeaderElection(
                    client, null, leaseId, keepAlive, electionPath, participantId);

            // Campaign asynchronously so the caller is never blocked when another
            // participant currently holds leadership.  campaign() blocks until this
            // node becomes the leader -- running it on a daemon thread allows the
            // method to return immediately with isLeader()==false and transition
            // later when leadership is actually acquired.
            Thread campaignThread = new Thread(() -> {
                try {
                    CampaignResponse campaignResponse = client.getElectionClient()
                            .campaign(
                                    toByteSequence(electionPath),
                                    leaseId,
                                    toByteSequence(participantId)
                            )
                            .get();

                    if (election.isClosed()) {
                        return; // election was closed while waiting
                    }

                    election.setLeaderKey(campaignResponse.getLeader());
                    election.setLeader(true);
                    election.notifyListeners(true);

                    log.info("Participant {} became leader on path {}", participantId, electionPath);

                    // Start observing leadership changes asynchronously
                    startObserving(election, electionPath);
                } catch (Exception e) {
                    if (!election.isClosed()) {
                        log.error("Campaign failed for participant {} on {}", participantId, electionPath, e);
                    }
                }
            }, "etcd-campaign-" + participantId);
            campaignThread.setDaemon(true);
            campaignThread.start();

            return election;
        } catch (ExecutionException e) {
            throw mapException(e, electionPath);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted during leader election: " + electionPath, e);
        }
    }

    @Override
    public void close() throws IOException {
        log.debug("Closing EtcdKVStore");
        client.close();
    }

    // ---- Internal helpers ----

    /**
     * Converts a String key to a jetcd {@link ByteSequence} using UTF-8.
     */
    private static ByteSequence toByteSequence(String key) {
        return ByteSequence.from(key, StandardCharsets.UTF_8);
    }

    /**
     * Maps a jetcd {@link KeyValue} to a {@link KVEntry}.
     */
    private static KVEntry toEntry(KeyValue kv) {
        return new KVEntry(
                kv.getKey().toString(StandardCharsets.UTF_8),
                kv.getValue().getBytes(),
                kv.getModRevision(),
                kv.getCreateRevision()
        );
    }

    /**
     * Dispatches watch events from a jetcd {@link WatchResponse} to a {@link KVWatcher}.
     */
    private static void dispatchWatchEvents(WatchResponse watchResponse, KVWatcher watcher,
                                            String keyOrPrefix) {
        for (WatchEvent event : watchResponse.getEvents()) {
            try {
                KVWatchEvent.Type type = event.getEventType() == WatchEvent.EventType.PUT
                        ? KVWatchEvent.Type.PUT
                        : KVWatchEvent.Type.DELETE;

                KVEntry current = toEntry(event.getKeyValue());

                KVEntry previous = null;
                KeyValue prevKV = event.getPrevKV();
                if (prevKV != null && prevKV.getModRevision() != 0) {
                    previous = toEntry(prevKV);
                }

                watcher.onEvent(new KVWatchEvent(type, current, previous));
            } catch (Exception e) {
                log.error("Error dispatching watch event for {}", keyOrPrefix, e);
            }
        }
    }

    /**
     * Starts an asynchronous observer on the election path to detect leadership changes.
     */
    private void startObserving(EtcdLeaderElection election, String electionPath) {
        client.getElectionClient().observe(
                toByteSequence(electionPath),
                new Election.Listener() {
                    @Override
                    public void onNext(LeaderResponse response) {
                        if (election.isClosed()) {
                            return;
                        }
                        try {
                            String currentLeaderValue = response.getKv()
                                    .getValue().toString(StandardCharsets.UTF_8);
                            boolean isNowLeader = election.getParticipantId().equals(currentLeaderValue);
                            boolean wasLeader = election.isLeader();

                            if (isNowLeader != wasLeader) {
                                election.setLeader(isNowLeader);
                                election.notifyListeners(isNowLeader);

                                if (isNowLeader) {
                                    log.info("Participant {} became leader on path {}",
                                            election.getParticipantId(), electionPath);
                                } else {
                                    log.info("Participant {} lost leadership on path {}",
                                            election.getParticipantId(), electionPath);
                                }
                            }
                        } catch (Exception e) {
                            log.error("Error processing election observation for {}", electionPath, e);
                        }
                    }

                    @Override
                    public void onError(Throwable throwable) {
                        if (election.isClosed()) {
                            return;
                        }
                        log.error("Election observation error for {}", electionPath, throwable);
                        // On observation error, assume leadership is lost for safety
                        if (election.isLeader()) {
                            election.setLeader(false);
                            election.notifyListeners(false);
                        }
                    }

                    @Override
                    public void onCompleted() {
                        log.debug("Election observation completed for {}", electionPath);
                    }
                }
        );
    }

    /**
     * Revokes a lease, logging but suppressing any errors.
     */
    private void silentRevokeLease(long leaseId) {
        try {
            client.getLeaseClient().revoke(leaseId).get();
        } catch (Exception e) {
            log.warn("Failed to revoke lease {}: {}", leaseId, e.getMessage());
        }
    }

    /**
     * Maps jetcd and execution exceptions to {@link KVStoreException} with the appropriate code.
     *
     * <p>Exception mapping rules:</p>
     * <ul>
     *     <li>{@link CompactedException} -> CONNECTION_LOST</li>
     *     <li>{@link EtcdException} with UNAVAILABLE -> CONNECTION_LOST</li>
     *     <li>{@link EtcdException} with DEADLINE_EXCEEDED -> OPERATION_TIMEOUT</li>
     *     <li>{@link EtcdException} with PERMISSION_DENIED or UNAUTHENTICATED -> UNAUTHORIZED</li>
     *     <li>{@link ExecutionException} -> unwrap and remap the cause</li>
     *     <li>{@link InterruptedException} -> OPERATION_TIMEOUT (re-interrupts thread)</li>
     *     <li>All others -> INTERNAL_ERROR</li>
     * </ul>
     */
    private static KVStoreException mapException(Exception e, String key) {
        // Unwrap ExecutionException to get the real cause
        Throwable cause = e;
        if (e instanceof ExecutionException && e.getCause() != null) {
            cause = e.getCause();
        }

        if (cause instanceof CompactedException _) {
            return new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                    "Compacted revision while accessing key: " + key, e);
        }

        if (cause instanceof EtcdException etcdEx) {
            ErrorCode errorCode = etcdEx.getErrorCode();
            return switch (errorCode) {
                case UNAVAILABLE -> new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                        "etcd unavailable while accessing key: " + key, e);
                case DEADLINE_EXCEEDED -> new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Operation timed out for key: " + key, e);
                case PERMISSION_DENIED, UNAUTHENTICATED -> new KVStoreException(KVStoreException.Code.UNAUTHORIZED,
                        "Unauthorized access to key: " + key, e);
                case NOT_FOUND -> new KVStoreException(KVStoreException.Code.KEY_NOT_FOUND,
                        "Key not found: " + key, e);
                default -> new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "etcd error (" + errorCode + ") for key: " + key, e);
            };
        }

        if (cause instanceof InterruptedException) {
            Thread.currentThread().interrupt();
            return new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }

        return new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                "Internal error for key: " + key, e);
    }

    // ---- No-op StreamObserver for lease keep-alive ----

    /**
     * A no-op {@link io.grpc.stub.StreamObserver} that logs errors and ignores
     * normal keep-alive responses. Used for lease keep-alive background streams.
     */
    private static final class NoOpStreamObserver<T> implements io.grpc.stub.StreamObserver<T> {

        @Override
        public void onNext(T value) {
            // Keep-alive response received; no action needed
        }

        @Override
        public void onError(Throwable t) {
            log.warn("Lease keep-alive stream error: {}", t.getMessage());
        }

        @Override
        public void onCompleted() {
            log.debug("Lease keep-alive stream completed");
        }
    }

    // ---- LeaderElection wrapper over etcd election API ----

    /**
     * etcd-backed {@link LeaderElection} implementation that tracks leadership status
     * via the etcd election API and notifies registered listeners on state transitions.
     *
     * <p>Thread safety: leadership state is tracked via a volatile boolean. Listeners
     * are stored in a {@link CopyOnWriteArrayList} for safe concurrent iteration.</p>
     */
    private static final class EtcdLeaderElection implements LeaderElection {

        private final Client client;
        private volatile LeaderKey leaderKey;
        private final long leaseId;
        private final CloseableClient keepAlive;
        private final String electionPath;
        private final String participantId;
        private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
        private volatile boolean leader;
        private volatile boolean closed;

        EtcdLeaderElection(Client client, LeaderKey leaderKey, long leaseId,
                           CloseableClient keepAlive, String electionPath, String participantId) {
            this.client = client;
            this.leaderKey = leaderKey;
            this.leaseId = leaseId;
            this.keepAlive = keepAlive;
            this.electionPath = electionPath;
            this.participantId = participantId;
            this.listeners = new CopyOnWriteArrayList<>();
        }

        void setLeaderKey(LeaderKey leaderKey) {
            this.leaderKey = leaderKey;
        }

        @Override
        public boolean isLeader() {
            return leader;
        }

        @Override
        public String currentLeaderId() throws KVStoreException {
            if (closed) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Leader election is closed");
            }
            try {
                LeaderResponse response = client.getElectionClient()
                        .leader(toByteSequence(electionPath))
                        .get();
                return response.getKv().getValue().toString(StandardCharsets.UTF_8);
            } catch (ExecutionException e) {
                Throwable cause = e.getCause();
                if (cause instanceof EtcdException etcdEx
                        && etcdEx.getErrorCode() == ErrorCode.NOT_FOUND) {
                    throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                            "No leader currently elected on path: " + electionPath, e);
                }
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to determine current leader on path: " + electionPath, e);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Operation interrupted while querying leader on path: " + electionPath, e);
            }
        }

        @Override
        public void addListener(LeaderChangeListener listener) {
            Objects.requireNonNull(listener, "listener");
            listeners.add(listener);
        }

        @Override
        public void close() throws IOException {
            if (closed) {
                return;
            }
            closed = true;
            leader = false;

            // Resign from the election (leaderKey may be null if campaign hasn't completed)
            LeaderKey lk = leaderKey;
            if (lk != null) {
                try {
                    client.getElectionClient().resign(lk).get();
                    log.debug("Resigned from election on {}", electionPath);
                } catch (Exception e) {
                    log.error("Error resigning from election on {}", electionPath, e);
                }
            }

            // Cancel keep-alive
            keepAlive.close();

            // Revoke the lease
            try {
                client.getLeaseClient().revoke(leaseId).get();
                log.debug("Revoked lease {} for election on {}", leaseId, electionPath);
            } catch (Exception e) {
                log.error("Error revoking lease {} for election on {}", leaseId, electionPath, e);
            }
        }

        void setLeader(boolean leader) {
            this.leader = leader;
        }

        boolean isClosed() {
            return closed;
        }

        String getParticipantId() {
            return participantId;
        }

        void notifyListeners(boolean isLeader) {
            for (LeaderChangeListener listener : listeners) {
                try {
                    listener.onLeaderChange(isLeader);
                } catch (Exception e) {
                    log.error("Error notifying leader change listener", e);
                }
            }
        }
    }
}
