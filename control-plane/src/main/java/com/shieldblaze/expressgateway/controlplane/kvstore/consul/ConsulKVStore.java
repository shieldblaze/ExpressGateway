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
package com.shieldblaze.expressgateway.controlplane.kvstore.consul;

import com.orbitz.consul.Consul;
import com.orbitz.consul.ConsulException;
import com.orbitz.consul.KeyValueClient;
import com.orbitz.consul.SessionClient;
import com.orbitz.consul.cache.ConsulCache;
import com.orbitz.consul.cache.KVCache;
import com.orbitz.consul.model.kv.Value;
import com.orbitz.consul.model.session.ImmutableSession;
import com.orbitz.consul.model.session.Session;
import com.orbitz.consul.model.session.SessionCreatedResponse;
import com.orbitz.consul.option.ImmutablePutOptions;
import com.orbitz.consul.option.PutOptions;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatchEvent;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.io.IOException;
import java.net.ConnectException;
import java.net.SocketTimeoutException;

import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.Set;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Consul-backed {@link KVStore} implementation using the orbitz consul-client library.
 *
 * <p>Key mapping: KV keys in this store use leading {@code /} (e.g. {@code /config/clusters/c1}).
 * Consul's KV API does not use leading slashes, so this implementation strips the leading
 * {@code /} before passing keys to Consul and prepends it back when returning {@link KVEntry}
 * instances. Consul's {@code ModifyIndex} maps to {@link KVEntry#version()} and
 * {@code CreateIndex} maps to {@link KVEntry#createVersion()}.</p>
 *
 * <p>Thread safety: the underlying {@link Consul} client is thread-safe. Watch callbacks
 * and leader election listeners fire from background daemon threads managed by this class.
 * All public methods are safe for concurrent use.</p>
 */
@Log4j2
public final class ConsulKVStore implements KVStore {

    private static final String SESSION_TTL = "30s";
    private static final String SESSION_LOCK_DELAY = "15s";
    private static final long SESSION_RENEW_INTERVAL_MS = 10_000L;
    private static final int LOCK_ACQUIRE_TIMEOUT_SECONDS = 30;
    private static final int LOCK_ACQUIRE_RETRY_INTERVAL_MS = 500;
    private static final int LEADER_POLL_INTERVAL_MS = 5_000;

    private final Consul consul;
    private final AtomicBoolean closed = new AtomicBoolean(false);

    /**
     * Creates a new Consul-backed KV store.
     *
     * @param consul The Consul client instance (externally managed lifecycle)
     * @throws NullPointerException if {@code consul} is null
     */
    public ConsulKVStore(Consul consul) {
        this.consul = Objects.requireNonNull(consul, "consul");
    }

    @Override
    public Optional<KVEntry> get(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            KeyValueClient kv = consul.keyValueClient();
            Optional<Value> value = kv.getValue(stripLeadingSlash(key));
            return value.map(v -> toEntry(key, v));
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public List<KVEntry> list(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulPrefix = stripLeadingSlash(prefix);
            // Ensure prefix ends with / for hierarchical listing
            if (!consulPrefix.isEmpty() && !consulPrefix.endsWith("/")) {
                consulPrefix = consulPrefix + "/";
            }

            List<Value> values;
            try {
                values = kv.getValues(consulPrefix);
            } catch (ConsulException e) {
                if (e.getCode() == 404) {
                    return Collections.emptyList();
                }
                throw e;
            }

            if (values == null || values.isEmpty()) {
                return Collections.emptyList();
            }

            // Filter to direct children only: keys that are exactly one path segment
            // deeper than the normalized prefix.
            String normalizedPrefix = consulPrefix;
            List<KVEntry> entries = new ArrayList<>();
            for (Value v : values) {
                String vKey = v.getKey();
                if (isDirectChild(normalizedPrefix, vKey)) {
                    entries.add(toEntry("/" + vKey, v));
                }
            }
            return entries;
        } catch (Exception e) {
            throw mapException(e, prefix);
        }
    }

    @Override
    public List<KVEntry> listRecursive(String prefix) throws KVStoreException {
        Objects.requireNonNull(prefix, "prefix");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulPrefix = stripLeadingSlash(prefix);
            // Ensure prefix ends with / for hierarchical listing
            if (!consulPrefix.isEmpty() && !consulPrefix.endsWith("/")) {
                consulPrefix = consulPrefix + "/";
            }

            List<Value> values;
            try {
                // getValues with a prefix already returns all descendants (Consul uses ?recurse)
                values = kv.getValues(consulPrefix);
            } catch (ConsulException e) {
                if (e.getCode() == 404) {
                    return Collections.emptyList();
                }
                throw e;
            }

            if (values == null || values.isEmpty()) {
                return Collections.emptyList();
            }

            // Return all descendants, not just direct children
            List<KVEntry> entries = new ArrayList<>(values.size());
            for (Value v : values) {
                String vKey = v.getKey();
                // Skip the prefix key itself if it appears in results
                if (!vKey.equals(consulPrefix) && !vKey.equals(stripLeadingSlash(prefix))) {
                    entries.add(toEntry("/" + vKey, v));
                }
            }
            return entries;
        } catch (Exception e) {
            throw mapException(e, prefix);
        }
    }

    @Override
    public long put(String key, byte[] value) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulKey = stripLeadingSlash(key);
            // Use byte[] overload to avoid lossy string conversion for binary data.
            // DIST-1 note: putValue + getValue is non-atomic (TOCTOU window).
            // A concurrent writer between the two calls could cause us to return a
            // different ModifyIndex. This is a known Consul API limitation — the regular
            // PUT endpoint does not return the new ModifyIndex in the response.
            // Impact is low: callers using the returned version for subsequent CAS
            // will fail-fast on version mismatch (correct behavior).
            boolean success = kv.putValue(consulKey, value, 0, PutOptions.BLANK);
            if (!success) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Consul PUT returned false for key: " + key);
            }

            // Retrieve the new ModifyIndex after the write
            Optional<Value> updated = kv.getValue(consulKey);
            if (updated.isEmpty()) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Key not found immediately after PUT: " + key);
            }
            return updated.get().getModifyIndex();
        } catch (KVStoreException e) {
            throw e;
        } catch (Exception e) {
            throw mapException(e, key);
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
            KeyValueClient kv = consul.keyValueClient();
            String consulKey = stripLeadingSlash(key);

            // Consul CAS: PUT with cas= parameter.
            // cas=0 means create-if-absent (key must not exist).
            // cas=N means update-if-version-matches (ModifyIndex must equal N).
            // The CAS itself is atomic — only the version retrieval has a TOCTOU window.
            PutOptions putOptions = ImmutablePutOptions.builder()
                    .cas(expectedVersion)
                    .build();
            boolean success = kv.putValue(consulKey, value, 0, putOptions);
            if (!success) {
                throw new KVStoreException(KVStoreException.Code.VERSION_CONFLICT,
                        "CAS failed for key: " + key + ", expectedVersion=" + expectedVersion);
            }

            // Retrieve the new ModifyIndex after the successful CAS.
            // DIST-1 note: small TOCTOU window — a concurrent writer could cause us to
            // return a higher version. This is safe: the caller's next CAS will fail-fast
            // on version mismatch rather than silently succeed.
            Optional<Value> updated = kv.getValue(consulKey);
            if (updated.isEmpty()) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Key not found immediately after CAS: " + key);
            }
            return updated.get().getModifyIndex();
        } catch (KVStoreException e) {
            throw e;
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public boolean delete(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulKey = stripLeadingSlash(key);

            // Check existence first since Consul delete always succeeds
            Optional<Value> existing = kv.getValue(consulKey);
            if (existing.isEmpty()) {
                return false;
            }

            kv.deleteKey(consulKey);
            return true;
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public void deleteTree(String key) throws KVStoreException {
        Objects.requireNonNull(key, "key");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulKey = stripLeadingSlash(key);
            // Delete the exact key itself
            kv.deleteKey(consulKey);
            // Delete all children under key + "/" to avoid deleting siblings
            // (e.g., deleting "config/cluster" should not delete "config/cluster-backup")
            String childPrefix = consulKey.endsWith("/") ? consulKey : consulKey + "/";
            kv.deleteKeys(childPrefix);
            log.debug("Deleted tree under {}", key);
        } catch (Exception e) {
            throw mapException(e, key);
        }
    }

    @Override
    public Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(watcher, "watcher");
        try {
            KeyValueClient kv = consul.keyValueClient();
            String consulPrefix = stripLeadingSlash(keyOrPrefix);
            // Ensure prefix ends with / for consistent child key detection
            if (!consulPrefix.isEmpty() && !consulPrefix.endsWith("/")) {
                consulPrefix = consulPrefix + "/";
            }

            // Use the consul-client KVCache which implements blocking query long-polling
            // internally, comparing snapshots and notifying on changes.
            // watchSeconds controls the Consul blocking query long-poll timeout.
            // 55 seconds is just under the Consul default max of 5 minutes, reducing
            // unnecessary polling while staying responsive to changes.
            KVCache cache = KVCache.newCache(kv, consulPrefix, 55);

            // Track previous state to generate correct PUT/DELETE events with previousEntry
            final Map<String, Value> previousState = new HashMap<>();

            ConsulCache.Listener<String, Value> listener = newValues -> {
                try {
                    Map<String, Value> oldState;
                    synchronized (previousState) {
                        oldState = new HashMap<>(previousState);
                    }

                    // Detect PUTs (new or modified keys)
                    for (Map.Entry<String, Value> entry : newValues.entrySet()) {
                        String k = entry.getKey();
                        Value newVal = entry.getValue();
                        Value oldVal = oldState.get(k);

                        // Reconstruct the original key path from the Consul Value
                        String fullKey = "/" + newVal.getKey();

                        if (oldVal == null) {
                            // New key created
                            KVEntry current = toEntry(fullKey, newVal);
                            watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.PUT, current, null));
                        } else if (oldVal.getModifyIndex() != newVal.getModifyIndex()) {
                            // Existing key modified
                            KVEntry previous = toEntry(fullKey, oldVal);
                            KVEntry current = toEntry(fullKey, newVal);
                            watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.PUT, current, previous));
                        }
                    }

                    // Detect DELETEs (keys in old state but not in new)
                    Set<String> deletedKeys = new HashSet<>(oldState.keySet());
                    deletedKeys.removeAll(newValues.keySet());
                    for (String deletedKey : deletedKeys) {
                        Value oldVal = oldState.get(deletedKey);
                        String fullKey = "/" + oldVal.getKey();
                        KVEntry previous = toEntry(fullKey, oldVal);
                        watcher.onEvent(new KVWatchEvent(KVWatchEvent.Type.DELETE, null, previous));
                    }

                    // Update the tracked state
                    synchronized (previousState) {
                        previousState.clear();
                        previousState.putAll(newValues);
                    }
                } catch (Exception e) {
                    log.error("Error dispatching watch event for prefix {}", keyOrPrefix, e);
                }
            };

            cache.addListener(listener);
            cache.start();

            log.debug("Installed watch on {}", keyOrPrefix);

            return () -> {
                log.debug("Closing watch on {}", keyOrPrefix);
                cache.stop();
            };
        } catch (Exception e) {
            throw mapException(e, keyOrPrefix);
        }
    }

    @Override
    public Closeable acquireLock(String lockPath) throws KVStoreException {
        Objects.requireNonNull(lockPath, "lockPath");
        try {
            String consulLockPath = stripLeadingSlash(lockPath);

            // Create a session with TTL for the lock
            Session session = ImmutableSession.builder()
                    .name("lock-" + consulLockPath)
                    .ttl(SESSION_TTL)
                    .lockDelay(SESSION_LOCK_DELAY)
                    .behavior("delete")
                    .build();

            SessionClient sessionClient = consul.sessionClient();
            SessionCreatedResponse created = sessionClient.createSession(session);
            String sessionId = created.getId();
            log.debug("Created session {} for lock {}", sessionId, lockPath);

            // Start session renewal thread
            AtomicBoolean renewalActive = new AtomicBoolean(true);
            Thread renewalThread = new Thread(() -> {
                while (renewalActive.get() && !Thread.currentThread().isInterrupted()) {
                    try {
                        Thread.sleep(SESSION_RENEW_INTERVAL_MS);
                        if (renewalActive.get()) {
                            sessionClient.renewSession(sessionId);
                            log.trace("Renewed session {} for lock {}", sessionId, lockPath);
                        }
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        break;
                    } catch (Exception e) {
                        log.warn("Failed to renew session {} for lock {}", sessionId, lockPath, e);
                    }
                }
            }, "consul-session-renewal-" + sessionId.substring(0, 8));
            renewalThread.setDaemon(true);
            renewalThread.start();

            // Attempt to acquire the lock with retries up to the timeout
            KeyValueClient kv = consul.keyValueClient();
            long deadline = System.currentTimeMillis() + (LOCK_ACQUIRE_TIMEOUT_SECONDS * 1000L);
            boolean acquired = false;

            while (System.currentTimeMillis() < deadline) {
                acquired = kv.acquireLock(consulLockPath, sessionId);
                if (acquired) {
                    break;
                }
                try {
                    Thread.sleep(LOCK_ACQUIRE_RETRY_INTERVAL_MS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    // Clean up session before propagating
                    renewalActive.set(false);
                    renewalThread.interrupt();
                    destroySessionSafely(sessionClient, sessionId, lockPath);
                    throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                            "Lock acquisition interrupted: " + lockPath, e);
                }
            }

            if (!acquired) {
                // Clean up session on failure
                renewalActive.set(false);
                renewalThread.interrupt();
                destroySessionSafely(sessionClient, sessionId, lockPath);
                throw new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Failed to acquire lock within " + LOCK_ACQUIRE_TIMEOUT_SECONDS + "s: " + lockPath);
            }

            log.debug("Acquired lock on {}", lockPath);

            return () -> {
                renewalActive.set(false);
                renewalThread.interrupt();
                try {
                    kv.releaseLock(consulLockPath, sessionId);
                    log.debug("Released lock on {}", lockPath);
                } catch (Exception e) {
                    log.error("Error releasing lock on {}", lockPath, e);
                }
                destroySessionSafely(sessionClient, sessionId, lockPath);
            };
        } catch (KVStoreException e) {
            throw e;
        } catch (Exception e) {
            throw mapException(e, lockPath);
        }
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException {
        Objects.requireNonNull(electionPath, "electionPath");
        Objects.requireNonNull(participantId, "participantId");
        try {
            String consulElectionPath = stripLeadingSlash(electionPath);

            // Create a session for this election participant
            Session session = ImmutableSession.builder()
                    .name("leader-" + consulElectionPath + "-" + participantId)
                    .ttl(SESSION_TTL)
                    .lockDelay(SESSION_LOCK_DELAY)
                    .behavior("delete")
                    .build();

            SessionClient sessionClient = consul.sessionClient();
            SessionCreatedResponse created = sessionClient.createSession(session);
            String sessionId = created.getId();
            log.debug("Created session {} for leader election on {} (participant={})",
                    sessionId, electionPath, participantId);

            KeyValueClient kv = consul.keyValueClient();
            ConsulLeaderElection election = new ConsulLeaderElection(
                    kv, sessionClient, consulElectionPath, sessionId, participantId);
            election.start();

            log.debug("Started leader election on {} with participant {}", electionPath, participantId);
            return election;
        } catch (Exception e) {
            throw mapException(e, electionPath);
        }
    }

    @Override
    public void close() throws IOException {
        if (closed.compareAndSet(false, true)) {
            consul.destroy();
            log.debug("ConsulKVStore closed");
        }
    }

    // ---- Internal helpers ----

    /**
     * Strips a leading {@code /} from the key. Consul keys do not use leading slashes.
     */
    private static String stripLeadingSlash(String key) {
        if (key.startsWith("/")) {
            return key.substring(1);
        }
        return key;
    }

    /**
     * Determines if a key is a direct child of the given prefix.
     * A direct child has exactly one additional path segment after the prefix.
     *
     * @param prefix The normalized consul prefix (with trailing {@code /})
     * @param key    The key to test
     * @return {@code true} if key is a direct child of prefix
     */
    private static boolean isDirectChild(String prefix, String key) {
        if (!key.startsWith(prefix)) {
            return false;
        }
        // Equal to prefix itself (e.g. the prefix node) -- not a child
        if (key.equals(prefix)) {
            return false;
        }
        String remainder = key.substring(prefix.length());
        // Direct child has no further / separators (or only a trailing one)
        String trimmed = remainder.endsWith("/") ? remainder.substring(0, remainder.length() - 1) : remainder;
        return !trimmed.isEmpty() && !trimmed.contains("/");
    }

    /**
     * Converts a Consul {@link Value} to a {@link KVEntry}.
     * The key in the returned entry always has a leading {@code /}.
     */
    private static KVEntry toEntry(String key, Value value) {
        byte[] data = value.getValueAsBytes().orElse(new byte[0]);
        return new KVEntry(key, data, value.getModifyIndex(), value.getCreateIndex());
    }

    /**
     * Safely destroys a Consul session, logging errors instead of propagating them.
     */
    private static void destroySessionSafely(SessionClient sessionClient, String sessionId, String context) {
        try {
            sessionClient.destroySession(sessionId);
            log.debug("Destroyed session {} ({})", sessionId, context);
        } catch (Exception e) {
            log.error("Error destroying session {} ({})", sessionId, context, e);
        }
    }

    /**
     * Maps Consul and transport exceptions to {@link KVStoreException} with the appropriate code.
     */
    private static KVStoreException mapException(Exception e, String key) {
        if (e instanceof ConsulException ce) {
            int code = ce.getCode();
            if (code == 404) {
                return new KVStoreException(KVStoreException.Code.KEY_NOT_FOUND,
                        "Key not found: " + key, e);
            }
            if (code == 403 || code == 401) {
                return new KVStoreException(KVStoreException.Code.UNAUTHORIZED,
                        "Unauthorized access to key: " + key, e);
            }
            if (code == 500 || code == 503) {
                return new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                        "Consul server error while accessing key: " + key, e);
            }
        }

        // Walk the cause chain for transport-level exceptions
        Throwable cause = e;
        while (cause != null) {
            if (cause instanceof ConnectException || cause instanceof SocketTimeoutException) {
                return new KVStoreException(KVStoreException.Code.CONNECTION_LOST,
                        "Connection lost while accessing key: " + key, e);
            }
            if (cause instanceof TimeoutException) {
                return new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                        "Operation timed out for key: " + key, e);
            }
            cause = cause.getCause();
        }

        if (e instanceof InterruptedException) {
            Thread.currentThread().interrupt();
            return new KVStoreException(KVStoreException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }

        return new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                "Internal error for key: " + key, e);
    }

    // ---- LeaderElection implementation using Consul session + KV acquire ----

    private static final class ConsulLeaderElection implements LeaderElection {

        private final KeyValueClient kv;
        private final SessionClient sessionClient;
        private final String electionPath;
        private final String sessionId;
        private final String participantId;
        private final CopyOnWriteArrayList<LeaderChangeListener> listeners;
        private final AtomicBoolean leader = new AtomicBoolean(false);
        private final AtomicBoolean active = new AtomicBoolean(true);
        private Thread renewalThread;
        private Thread electionThread;

        ConsulLeaderElection(KeyValueClient kv, SessionClient sessionClient,
                             String electionPath, String sessionId, String participantId) {
            this.kv = kv;
            this.sessionClient = sessionClient;
            this.electionPath = electionPath;
            this.sessionId = sessionId;
            this.participantId = participantId;
            this.listeners = new CopyOnWriteArrayList<>();
        }

        void start() {
            // Session renewal thread
            renewalThread = new Thread(() -> {
                while (active.get() && !Thread.currentThread().isInterrupted()) {
                    try {
                        Thread.sleep(SESSION_RENEW_INTERVAL_MS);
                        if (active.get()) {
                            sessionClient.renewSession(sessionId);
                            log.trace("Renewed election session {}", sessionId);
                        }
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        break;
                    } catch (Exception e) {
                        log.warn("Failed to renew election session {}", sessionId, e);
                        // If session renewal fails, we may lose leadership
                        updateLeaderState(false);
                    }
                }
            }, "consul-election-renewal-" + sessionId.substring(0, 8));
            renewalThread.setDaemon(true);
            renewalThread.start();

            // Election polling thread: attempt to acquire leadership and monitor status
            electionThread = new Thread(() -> {
                while (active.get() && !Thread.currentThread().isInterrupted()) {
                    try {
                        // Attempt to acquire the election key
                        boolean acquired = kv.acquireLock(electionPath, participantId, sessionId);
                        if (acquired) {
                            updateLeaderState(true);
                        } else {
                            // Check if we currently hold the lock
                            Optional<Value> value = kv.getValue(electionPath);
                            boolean isCurrentLeader = value.isPresent()
                                    && value.get().getSession().isPresent()
                                    && value.get().getSession().get().equals(sessionId);
                            updateLeaderState(isCurrentLeader);
                        }

                        Thread.sleep(LEADER_POLL_INTERVAL_MS);
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        break;
                    } catch (Exception e) {
                        log.warn("Error during leader election poll for path {}", electionPath, e);
                        updateLeaderState(false);
                        try {
                            Thread.sleep(LEADER_POLL_INTERVAL_MS);
                        } catch (InterruptedException ie) {
                            Thread.currentThread().interrupt();
                            break;
                        }
                    }
                }
            }, "consul-election-poll-" + sessionId.substring(0, 8));
            electionThread.setDaemon(true);
            electionThread.start();
        }

        private void updateLeaderState(boolean isNowLeader) {
            boolean wasLeader = leader.getAndSet(isNowLeader);
            if (wasLeader != isNowLeader) {
                if (isNowLeader) {
                    log.info("Participant {} became leader on path {}", participantId, electionPath);
                } else {
                    log.info("Participant {} lost leadership on path {}", participantId, electionPath);
                }
                notifyListeners(isNowLeader);
            }
        }

        @Override
        public boolean isLeader() {
            return leader.get();
        }

        @Override
        public String currentLeaderId() throws KVStoreException {
            try {
                Optional<Value> value = kv.getValue(electionPath);
                if (value.isEmpty() || value.get().getSession().isEmpty()) {
                    throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                            "No leader currently elected on path: " + electionPath);
                }
                // The value stored at the election key is the participant ID of the leader
                // (written by acquireLock with the participantId as value)
                Optional<String> leaderValue = value.get().getValueAsString();
                if (leaderValue.isPresent() && !leaderValue.get().isEmpty()) {
                    return leaderValue.get();
                }
                // Fallback: return the session ID holding the lock
                return value.get().getSession().get();
            } catch (KVStoreException e) {
                throw e;
            } catch (Exception e) {
                throw new KVStoreException(KVStoreException.Code.INTERNAL_ERROR,
                        "Failed to determine current leader on path: " + electionPath, e);
            }
        }

        @Override
        public void addListener(LeaderChangeListener listener) {
            Objects.requireNonNull(listener, "listener");
            listeners.add(listener);
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

        @Override
        public void close() throws IOException {
            if (active.compareAndSet(true, false)) {
                // Stop background threads
                if (renewalThread != null) {
                    renewalThread.interrupt();
                }
                if (electionThread != null) {
                    electionThread.interrupt();
                }

                // Release the lock if we held it
                try {
                    kv.releaseLock(electionPath, sessionId);
                } catch (Exception e) {
                    log.error("Error releasing election lock on {}", electionPath, e);
                }

                // Destroy the session
                destroySessionSafely(sessionClient, sessionId, "election-" + electionPath);

                // Notify listeners of leadership loss
                if (leader.getAndSet(false)) {
                    notifyListeners(false);
                }
            }
        }
    }
}
