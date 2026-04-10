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
package com.shieldblaze.expressgateway.coordination.consul;

import com.google.common.net.HostAndPort;
import com.orbitz.consul.Consul;
import com.orbitz.consul.ConsulException;
import com.orbitz.consul.KeyValueClient;
import com.orbitz.consul.SessionClient;
import com.orbitz.consul.cache.ConsulCache;
import com.orbitz.consul.cache.KVCache;
import com.orbitz.consul.model.kv.Value;
import com.orbitz.consul.model.session.ImmutableSession;
import com.orbitz.consul.model.session.SessionCreatedResponse;
import com.orbitz.consul.option.ImmutablePutOptions;
import com.orbitz.consul.option.PutOptions;
import com.shieldblaze.expressgateway.coordination.ConnectionListener;
import com.shieldblaze.expressgateway.coordination.CoordinationEntry;
import com.shieldblaze.expressgateway.coordination.CoordinationException;
import com.shieldblaze.expressgateway.coordination.CoordinationProvider;
import com.shieldblaze.expressgateway.coordination.DistributedLock;
import com.shieldblaze.expressgateway.coordination.LeaderElection;
import com.shieldblaze.expressgateway.coordination.WatchEvent;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;

/**
 * Consul-backed {@link CoordinationProvider} implementation using the orbitz consul-client 1.5.3.
 *
 * <h2>Key format</h2>
 * <p>The coordination API uses keys with leading {@code /} (e.g. {@code /expressgateway/config}).
 * Consul KV keys do NOT start with {@code /}, so this implementation strips the leading slash
 * on all operations and re-adds it when building {@link CoordinationEntry} instances.</p>
 *
 * <h2>Version mapping</h2>
 * <p>Consul's {@code ModifyIndex} serves as the key version. Unlike ZooKeeper where versions
 * start at 0, Consul's {@code ModifyIndex} is a global Raft index that is always >= 1 for
 * existing keys, so no offset adjustment is needed. {@code CreateIndex} maps to
 * {@code createVersion}.</p>
 *
 * <h2>Ephemeral keys</h2>
 * <p>Implemented via Consul sessions with TTL and "delete" behavior. When the session expires
 * or is destroyed, all keys associated with it are automatically deleted.</p>
 *
 * <h2>Sequential keys</h2>
 * <p>Consul has no native sequential key support. This is implemented via a CAS counter key
 * at {@code {prefix}__counter} that is atomically incremented, producing monotonically
 * increasing suffixes.</p>
 *
 * <h2>Watches</h2>
 * <p>Implemented using the consul-client's {@link KVCache}, which internally uses Consul's
 * blocking query mechanism (long-poll with index) for efficient change detection.</p>
 *
 * <h2>Thread safety</h2>
 * <p>The underlying {@link Consul} client is thread-safe. Connection listeners use
 * {@link CopyOnWriteArrayList}. Background health monitoring runs on a virtual thread.</p>
 *
 * <h2>Ownership</h2>
 * <p>When constructed via the static factories {@link #create(String, int)} or
 * {@link #create(String, int, String)}, this provider owns the Consul client and will
 * destroy it on {@link #close()}. When constructed with an externally-provided client,
 * the caller retains ownership.</p>
 */
@Log4j2
public final class ConsulCoordinationProvider implements CoordinationProvider {

    private static final String EPHEMERAL_SESSION_TTL = "30s";
    private static final long LOCK_SESSION_TTL_SECONDS = 30;
    private static final String LOCK_SESSION_TTL = LOCK_SESSION_TTL_SECONDS + "s";
    private static final String LOCK_DELAY = "15s";
    private static final long HEALTH_CHECK_INTERVAL_MS = 5_000;
    private static final int SEQUENTIAL_MAX_RETRIES = 10;
    private static final int LOCK_POLL_INTERVAL_MS = 500;

    private final Consul consul;
    private final boolean ownsClient;
    private final CopyOnWriteArrayList<ConnectionListener> connectionListeners;
    private final AtomicBoolean connected;
    private volatile Thread healthCheckThread;

    /**
     * Creates a provider wrapping an externally-managed Consul client.
     * The caller retains ownership and must destroy the client separately.
     *
     * @param consul an already-built Consul client instance
     */
    public ConsulCoordinationProvider(Consul consul) {
        this(consul, false);
    }

    /**
     * Package-private constructor controlling ownership semantics.
     *
     * @param consul     the Consul client instance
     * @param ownsClient if true, close() will also destroy the Consul client
     */
    ConsulCoordinationProvider(Consul consul, boolean ownsClient) {
        this.consul = Objects.requireNonNull(consul, "consul");
        this.ownsClient = ownsClient;
        this.connectionListeners = new CopyOnWriteArrayList<>();
        this.connected = new AtomicBoolean(true);

        // Start background health monitoring
        healthCheckThread = Thread.ofVirtual()
                .name("consul-health-monitor")
                .start(this::healthCheckLoop);
    }

    /**
     * Convenience factory that creates a Consul client and returns a provider that owns it.
     *
     * @param host the Consul agent host
     * @param port the Consul agent HTTP port
     * @return a fully connected provider
     */
    public static ConsulCoordinationProvider create(String host, int port) {
        Consul consul = Consul.builder()
                .withHostAndPort(HostAndPort.fromParts(host, port))
                .build();
        return new ConsulCoordinationProvider(consul, true);
    }

    /**
     * Convenience factory that creates a Consul client with ACL token authentication.
     *
     * @param host     the Consul agent host
     * @param port     the Consul agent HTTP port
     * @param aclToken the ACL token for authentication
     * @return a fully connected provider
     */
    public static ConsulCoordinationProvider create(String host, int port, String aclToken) {
        Consul consul = Consul.builder()
                .withHostAndPort(HostAndPort.fromParts(host, port))
                .withAclToken(aclToken)
                .build();
        return new ConsulCoordinationProvider(consul, true);
    }

    // ---- Key-Value CRUD ----

    @Override
    public Optional<CoordinationEntry> get(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        String consulKey = toConsulKey(key);
        try {
            Optional<Value> valueOpt = consul.keyValueClient().getValue(consulKey);
            return valueOpt.map(v -> toEntry(key, v));
        } catch (ConsulException e) {
            if (isNotFound(e)) {
                return Optional.empty();
            }
            throw mapException(e, key);
        }
    }

    @Override
    public long put(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        String consulKey = toConsulKey(key);
        try {
            KeyValueClient kvClient = consul.keyValueClient();
            boolean success = kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8));
            if (!success) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "Consul put returned false for key: " + key);
            }
            // Read back to get the ModifyIndex as the version
            Optional<Value> valueOpt = kvClient.getValue(consulKey);
            if (valueOpt.isEmpty()) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "Key disappeared immediately after put: " + key);
            }
            return valueOpt.get().getModifyIndex();
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw mapException(e, key);
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
        String consulKey = toConsulKey(key);
        try {
            KeyValueClient kvClient = consul.keyValueClient();

            if (expectedVersion == 0) {
                // Create-if-absent: use CAS with index 0, which in Consul means "put only if key does not exist"
                PutOptions opts = ImmutablePutOptions.builder()
                        .cas(0L)
                        .build();
                boolean success = kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8), 0, opts);
                if (!success) {
                    // Key already exists
                    throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                            "Key already exists (CAS with expectedVersion=0): " + key);
                }
            } else {
                // CAS update with expected ModifyIndex
                PutOptions opts = ImmutablePutOptions.builder()
                        .cas(expectedVersion)
                        .build();
                boolean success = kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8), 0, opts);
                if (!success) {
                    // Check if the key exists to distinguish VERSION_CONFLICT vs KEY_NOT_FOUND
                    Optional<Value> existing = kvClient.getValue(consulKey);
                    if (existing.isEmpty()) {
                        throw new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                                "Key not found for CAS update: " + key);
                    }
                    throw new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                            "Version mismatch for key: " + key + ", expectedVersion=" + expectedVersion
                                    + ", actualVersion=" + existing.get().getModifyIndex());
                }
            }
            // Read back to get the new ModifyIndex
            Optional<Value> updated = kvClient.getValue(consulKey);
            if (updated.isEmpty()) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "Key disappeared immediately after CAS: " + key);
            }
            return updated.get().getModifyIndex();
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw mapException(e, key);
        }
    }

    @Override
    public boolean delete(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        String consulKey = toConsulKey(key);
        try {
            KeyValueClient kvClient = consul.keyValueClient();
            // Check existence first since Consul delete is always void
            Optional<Value> existing = kvClient.getValue(consulKey);
            if (existing.isEmpty()) {
                return false;
            }
            kvClient.deleteKey(consulKey);
            return true;
        } catch (ConsulException e) {
            if (isNotFound(e)) {
                return false;
            }
            throw mapException(e, key);
        }
    }

    @Override
    public void deleteTree(String key) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        String consulPrefix = toConsulKey(normalizePrefix(key));
        try {
            consul.keyValueClient().deleteKeys(consulPrefix);
        } catch (ConsulException e) {
            if (isNotFound(e)) {
                return; // Idempotent
            }
            throw mapException(e, key);
        }
    }

    @Override
    public List<CoordinationEntry> list(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePrefix(prefix);
        String consulPrefix = toConsulKey(normalizedPrefix);
        // Ensure the prefix ends with '/' for direct-children filtering
        String searchPrefix = consulPrefix.endsWith("/") ? consulPrefix : consulPrefix + "/";
        int searchDepth = countSlashes(searchPrefix);

        try {
            List<Value> values = consul.keyValueClient().getValues(searchPrefix);
            if (values == null || values.isEmpty()) {
                return Collections.emptyList();
            }
            List<CoordinationEntry> entries = new ArrayList<>();
            for (Value value : values) {
                String valueKey = value.getKey();
                // Direct children only: depth is exactly searchDepth (one level deeper)
                int valueDepth = countSlashes(valueKey);
                // A direct child has the same number of slashes as the search prefix,
                // or one more if the child key itself doesn't end with '/'
                if (isDirectChild(searchPrefix, valueKey)) {
                    entries.add(toEntry(toApiKey(valueKey), value));
                }
            }
            return entries;
        } catch (ConsulException e) {
            if (isNotFound(e)) {
                return Collections.emptyList();
            }
            throw mapException(e, prefix);
        }
    }

    @Override
    public List<CoordinationEntry> listRecursive(String prefix) throws CoordinationException {
        Objects.requireNonNull(prefix, "prefix");
        String normalizedPrefix = normalizePrefix(prefix);
        String consulPrefix = toConsulKey(normalizedPrefix);
        String searchPrefix = consulPrefix.endsWith("/") ? consulPrefix : consulPrefix + "/";

        try {
            List<Value> values = consul.keyValueClient().getValues(searchPrefix);
            if (values == null || values.isEmpty()) {
                return Collections.emptyList();
            }
            List<CoordinationEntry> entries = new ArrayList<>(values.size());
            for (Value value : values) {
                entries.add(toEntry(toApiKey(value.getKey()), value));
            }
            return entries;
        } catch (ConsulException e) {
            if (isNotFound(e)) {
                return Collections.emptyList();
            }
            throw mapException(e, prefix);
        }
    }

    // ---- Ephemeral and Sequential keys ----

    @Override
    public long putEphemeral(String key, byte[] value) throws CoordinationException {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
        String consulKey = toConsulKey(key);
        try {
            // Create a session with TTL and "delete" behavior so the key is auto-removed on expiry
            ImmutableSession session = ImmutableSession.builder()
                    .name("ephemeral-" + consulKey)
                    .ttl(EPHEMERAL_SESSION_TTL)
                    .behavior("delete")
                    .build();
            SessionCreatedResponse sessionResponse = consul.sessionClient().createSession(session);
            String sessionId = sessionResponse.getId();

            // Put the value with session acquire
            PutOptions opts = ImmutablePutOptions.builder()
                    .acquire(sessionId)
                    .build();
            KeyValueClient kvClient = consul.keyValueClient();
            boolean success = kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8), 0, opts);
            if (!success) {
                // Key already locked by another session -- overwrite without acquire first, then acquire
                kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8));
                success = kvClient.putValue(consulKey, new String(value, StandardCharsets.UTF_8), 0, opts);
                if (!success) {
                    consul.sessionClient().destroySession(sessionId);
                    throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                            "Failed to put ephemeral key: " + key);
                }
            }

            // Start a background session renewal thread
            Thread.ofVirtual()
                    .name("consul-ephemeral-renewal-" + consulKey)
                    .start(() -> ephemeralRenewalLoop(sessionId));

            Optional<Value> updated = kvClient.getValue(consulKey);
            if (updated.isEmpty()) {
                throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                        "Ephemeral key disappeared immediately after put: " + key);
            }
            return updated.get().getModifyIndex();
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw mapException(e, key);
        }
    }

    @Override
    public String putSequential(String keyPrefix, byte[] value) throws CoordinationException {
        Objects.requireNonNull(keyPrefix, "keyPrefix");
        Objects.requireNonNull(value, "value");
        String consulPrefix = toConsulKey(keyPrefix);
        String counterKey = consulPrefix + "__counter";

        try {
            KeyValueClient kvClient = consul.keyValueClient();

            for (int attempt = 0; attempt < SEQUENTIAL_MAX_RETRIES; attempt++) {
                long nextSeq;

                Optional<Value> counterOpt = kvClient.getValue(counterKey);
                if (counterOpt.isEmpty()) {
                    // Counter does not exist -- try to create it with CAS=0 (create-if-absent)
                    PutOptions opts = ImmutablePutOptions.builder().cas(0L).build();
                    boolean created = kvClient.putValue(counterKey, "1", 0, opts);
                    if (created) {
                        nextSeq = 1;
                    } else {
                        // Race: someone else created it -- retry
                        continue;
                    }
                } else {
                    Value counterValue = counterOpt.get();
                    long currentSeq = Long.parseLong(counterValue.getValueAsString().orElse("0"));
                    nextSeq = currentSeq + 1;
                    PutOptions opts = ImmutablePutOptions.builder()
                            .cas(counterValue.getModifyIndex())
                            .build();
                    boolean updated = kvClient.putValue(counterKey, String.valueOf(nextSeq), 0, opts);
                    if (!updated) {
                        // CAS conflict -- retry
                        continue;
                    }
                }

                // Counter incremented successfully -- write the sequential value
                String seqSuffix = String.format("%010d", nextSeq);
                String seqKey = consulPrefix + seqSuffix;
                kvClient.putValue(seqKey, new String(value, StandardCharsets.UTF_8));
                return toApiKey(seqKey);
            }

            throw new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                    "Failed to create sequential key after " + SEQUENTIAL_MAX_RETRIES
                            + " attempts due to contention: " + keyPrefix);
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw mapException(e, keyPrefix);
        }
    }

    // ---- Watch ----

    @Override
    public Closeable watch(String keyOrPrefix, Consumer<WatchEvent> listener) throws CoordinationException {
        Objects.requireNonNull(keyOrPrefix, "keyOrPrefix");
        Objects.requireNonNull(listener, "listener");
        String consulKey = toConsulKey(normalizePrefix(keyOrPrefix));

        try {
            KVCache kvCache = KVCache.newCache(consul.keyValueClient(), consulKey);

            // Track previous state for computing diffs
            Map<String, Value> previousState = new HashMap<>();

            ConsulCache.Listener<String, Value> cacheListener = newValues -> {
                try {
                    // Detect puts (new or changed)
                    for (Map.Entry<String, Value> entry : newValues.entrySet()) {
                        String k = entry.getKey();
                        Value newValue = entry.getValue();
                        Value oldValue = previousState.get(k);

                        if (oldValue == null) {
                            // New key created
                            CoordinationEntry current = toEntry(toApiKey(consulKey + "/" + k), newValue);
                            listener.accept(new WatchEvent(WatchEvent.Type.PUT, current, null));
                        } else if (oldValue.getModifyIndex() != newValue.getModifyIndex()) {
                            // Existing key changed
                            CoordinationEntry previous = toEntry(toApiKey(consulKey + "/" + k), oldValue);
                            CoordinationEntry current = toEntry(toApiKey(consulKey + "/" + k), newValue);
                            listener.accept(new WatchEvent(WatchEvent.Type.PUT, current, previous));
                        }
                    }

                    // Detect deletes
                    for (Map.Entry<String, Value> entry : previousState.entrySet()) {
                        if (!newValues.containsKey(entry.getKey())) {
                            CoordinationEntry deleted = toEntry(
                                    toApiKey(consulKey + "/" + entry.getKey()), entry.getValue());
                            listener.accept(new WatchEvent(WatchEvent.Type.DELETE, null, deleted));
                        }
                    }

                    // Update tracked state
                    previousState.clear();
                    previousState.putAll(newValues);
                } catch (Exception e) {
                    log.error("Error dispatching watch event for prefix {}", consulKey, e);
                }
            };

            kvCache.addListener(cacheListener);
            kvCache.start();

            log.debug("Installed watch on {}", keyOrPrefix);

            return () -> {
                log.debug("Closing watch on {}", keyOrPrefix);
                kvCache.stop();
            };
        } catch (ConsulException e) {
            throw mapException(e, keyOrPrefix);
        }
    }

    // ---- Leader election ----

    @Override
    public LeaderElection leaderElection(String path, String participantId) throws CoordinationException {
        Objects.requireNonNull(path, "path");
        Objects.requireNonNull(participantId, "participantId");
        String consulPath = toConsulKey(path);
        return new ConsulLeaderElection(consul, consulPath, participantId);
    }

    // ---- Distributed locking ----

    @Override
    public DistributedLock acquireLock(String lockPath, long timeout, TimeUnit unit) throws CoordinationException {
        Objects.requireNonNull(lockPath, "lockPath");
        Objects.requireNonNull(unit, "unit");
        String consulLockPath = toConsulKey(lockPath);

        try {
            // Create a session with lock-delay and "release" behavior
            ImmutableSession session = ImmutableSession.builder()
                    .name("lock-" + consulLockPath)
                    .ttl(LOCK_SESSION_TTL)
                    .lockDelay(LOCK_DELAY)
                    .behavior("release")
                    .build();
            SessionCreatedResponse sessionResponse = consul.sessionClient().createSession(session);
            String sessionId = sessionResponse.getId();

            long deadlineNanos = System.nanoTime() + unit.toNanos(timeout);

            // Poll until we acquire the lock or timeout
            while (System.nanoTime() < deadlineNanos) {
                boolean acquired = consul.keyValueClient().acquireLock(consulLockPath, sessionId);
                if (acquired) {
                    log.debug("Acquired lock on {} (session={})", lockPath, sessionId);
                    return new ConsulDistributedLock(consul, consulLockPath, sessionId);
                }

                long remainingMs = TimeUnit.NANOSECONDS.toMillis(deadlineNanos - System.nanoTime());
                if (remainingMs <= 0) {
                    break;
                }
                try {
                    Thread.sleep(Math.min(LOCK_POLL_INTERVAL_MS, remainingMs));
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                    consul.sessionClient().destroySession(sessionId);
                    throw new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                            "Interrupted while acquiring lock: " + lockPath, e);
                }
            }

            // Timed out -- clean up the session
            consul.sessionClient().destroySession(sessionId);
            throw new CoordinationException(CoordinationException.Code.LOCK_ACQUISITION_FAILED,
                    "Failed to acquire lock within " + timeout + " " + unit + ": " + lockPath);
        } catch (CoordinationException e) {
            throw e;
        } catch (ConsulException e) {
            throw mapException(e, lockPath);
        }
    }

    // ---- Connection health ----

    @Override
    public boolean isConnected() {
        try {
            consul.agentClient().ping();
            return true;
        } catch (Exception e) {
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
        log.debug("Closing ConsulCoordinationProvider (ownsClient={})", ownsClient);
        Thread ht = healthCheckThread;
        if (ht != null) {
            ht.interrupt();
        }
        if (ownsClient) {
            consul.destroy();
        }
    }

    // ---- Internal helpers ----

    /**
     * Strips the leading '/' from a key to produce a valid Consul KV key.
     * Consul keys do not start with '/'.
     */
    static String toConsulKey(String apiKey) {
        if (apiKey.startsWith("/")) {
            return apiKey.substring(1);
        }
        return apiKey;
    }

    /**
     * Prepends '/' to a Consul key to produce an API-compatible key.
     */
    static String toApiKey(String consulKey) {
        if (consulKey.startsWith("/")) {
            return consulKey;
        }
        return "/" + consulKey;
    }

    /**
     * Normalizes a prefix path by stripping trailing slash for consistent handling.
     */
    private static String normalizePrefix(String path) {
        if (path.length() > 1 && path.endsWith("/")) {
            return path.substring(0, path.length() - 1);
        }
        return path;
    }

    /**
     * Counts the number of '/' characters in a string.
     */
    private static int countSlashes(String s) {
        int count = 0;
        for (int i = 0; i < s.length(); i++) {
            if (s.charAt(i) == '/') {
                count++;
            }
        }
        return count;
    }

    /**
     * Determines if a key is a direct child of the given prefix.
     * A direct child is one level below the prefix with no additional '/' separators.
     */
    private static boolean isDirectChild(String prefix, String key) {
        if (!key.startsWith(prefix) || key.equals(prefix)) {
            return false;
        }
        String remainder = key.substring(prefix.length());
        // Strip trailing slash from remainder if present (Consul folder keys end with '/')
        if (remainder.endsWith("/")) {
            remainder = remainder.substring(0, remainder.length() - 1);
        }
        // Direct child has no additional '/' in the remainder
        return !remainder.isEmpty() && !remainder.contains("/");
    }

    /**
     * Converts a Consul {@link Value} to a {@link CoordinationEntry}.
     */
    private static CoordinationEntry toEntry(String apiKey, Value value) {
        byte[] data = value.getValueAsBytes().orElse(new byte[0]);
        return new CoordinationEntry(
                apiKey,
                data,
                value.getModifyIndex(),
                value.getCreateIndex()
        );
    }

    /**
     * Checks if a ConsulException represents a 404 Not Found.
     */
    private static boolean isNotFound(ConsulException e) {
        return e.getCode() == 404;
    }

    /**
     * Maps Consul exceptions to {@link CoordinationException} with appropriate codes.
     */
    static CoordinationException mapException(Exception e, String key) {
        if (e instanceof ConsulException consulEx) {
            int code = consulEx.getCode();
            if (code == 404) {
                return new CoordinationException(CoordinationException.Code.KEY_NOT_FOUND,
                        "Key not found: " + key, e);
            }
            if (code == 409) {
                return new CoordinationException(CoordinationException.Code.VERSION_CONFLICT,
                        "Version conflict for key: " + key, e);
            }
            if (code == 500 || code == 503) {
                return new CoordinationException(CoordinationException.Code.CONNECTION_LOST,
                        "Connection lost while accessing key: " + key, e);
            }
        }
        if (e instanceof java.net.SocketTimeoutException
                || e instanceof java.util.concurrent.TimeoutException) {
            return new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Operation timed out for key: " + key, e);
        }
        if (e instanceof InterruptedException) {
            Thread.currentThread().interrupt();
            return new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Operation interrupted for key: " + key, e);
        }
        // Check if the cause is a timeout (Consul client wraps timeouts in ConsulException)
        if (e.getCause() instanceof java.net.SocketTimeoutException) {
            return new CoordinationException(CoordinationException.Code.OPERATION_TIMEOUT,
                    "Operation timed out for key: " + key, e);
        }
        return new CoordinationException(CoordinationException.Code.INTERNAL_ERROR,
                "Internal error for key: " + key, e);
    }

    /**
     * Background loop that periodically pings the Consul agent and fires connection state
     * change events to registered listeners.
     */
    private void healthCheckLoop() {
        while (!Thread.currentThread().isInterrupted()) {
            try {
                Thread.sleep(HEALTH_CHECK_INTERVAL_MS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
            try {
                consul.agentClient().ping();
                if (connected.compareAndSet(false, true)) {
                    fireConnectionStateChange(ConnectionListener.ConnectionState.RECONNECTED);
                }
            } catch (Exception e) {
                if (connected.compareAndSet(true, false)) {
                    fireConnectionStateChange(ConnectionListener.ConnectionState.LOST);
                }
            }
        }
    }

    /**
     * Notifies all registered connection listeners of a state change.
     */
    private void fireConnectionStateChange(ConnectionListener.ConnectionState state) {
        for (ConnectionListener listener : connectionListeners) {
            try {
                listener.onConnectionStateChange(state);
            } catch (Exception e) {
                log.error("Error notifying connection listener of state {}", state, e);
            }
        }
    }

    /**
     * Background loop that renews an ephemeral session to keep the associated key alive.
     * Runs until the session is destroyed or the thread is interrupted.
     */
    private void ephemeralRenewalLoop(String sessionId) {
        SessionClient sessionClient = consul.sessionClient();
        // Renew at roughly TTL/3 to stay well within the window
        long renewalIntervalMs = 10_000;
        while (!Thread.currentThread().isInterrupted()) {
            try {
                Thread.sleep(renewalIntervalMs);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }
            try {
                Optional<?> renewed = sessionClient.renewSession(sessionId);
                if (renewed.isEmpty()) {
                    // Session expired -- stop renewal
                    log.debug("Ephemeral session {} expired, stopping renewal", sessionId);
                    break;
                }
            } catch (ConsulException e) {
                log.debug("Failed to renew ephemeral session {}, it may have expired", sessionId, e);
                break;
            }
        }
    }
}
