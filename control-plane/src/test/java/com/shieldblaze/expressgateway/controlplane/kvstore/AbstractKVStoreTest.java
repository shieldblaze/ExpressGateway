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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.io.Closeable;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.UUID;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Abstract test suite for the {@link KVStore} interface.
 *
 * <p>Each backend (etcd, Consul, ZooKeeper) extends this class and provides
 * a concrete KVStore instance via Testcontainers. This ensures ALL backends
 * pass the SAME test suite, preventing behavioral drift between implementations.</p>
 *
 * <p>Every test uses a unique key prefix derived from the test method name and a
 * random UUID to guarantee isolation. The {@link #cleanupKeys} list is used to
 * clean up all keys after each test.</p>
 */
public abstract class AbstractKVStoreTest {

    /**
     * Keys and prefixes created during each test, cleaned up in {@link #cleanup()}.
     */
    private final List<String> cleanupKeys = new ArrayList<>();

    /**
     * Returns the KVStore instance under test, backed by a real container.
     */
    protected abstract KVStore kvStore();

    /**
     * Generates a unique key prefix for the current test to ensure isolation.
     */
    private String uniquePrefix() {
        String prefix = "/test/" + UUID.randomUUID().toString().substring(0, 8);
        cleanupKeys.add(prefix);
        return prefix;
    }

    @AfterEach
    void cleanup() throws KVStoreException {
        for (String key : cleanupKeys) {
            try {
                kvStore().deleteTree(key);
            } catch (KVStoreException e) {
                // Best-effort cleanup -- ignore failures
            }
        }
        cleanupKeys.clear();
    }

    // === CRUD Tests ===

    @Test
    @Timeout(60)
    void testPutAndGet() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/mykey";
        byte[] value = "hello-world".getBytes(StandardCharsets.UTF_8);

        long version = kvStore().put(key, value);
        assertTrue(version >= 0, "Version after put must be >= 0, got: " + version);

        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isPresent(), "Expected key to exist after put");
        assertEquals(key, entry.get().key());
        assertArrayEquals(value, entry.get().value());
        assertTrue(entry.get().version() >= 0, "Version must be >= 0");
    }

    @Test
    @Timeout(60)
    void testGetNonExistent() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/nonexistent";

        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isEmpty(), "Expected empty Optional for nonexistent key");
    }

    @Test
    @Timeout(60)
    void testPutOverwrite() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/overwrite";

        byte[] value1 = "first".getBytes(StandardCharsets.UTF_8);
        byte[] value2 = "second".getBytes(StandardCharsets.UTF_8);

        long version1 = kvStore().put(key, value1);
        long version2 = kvStore().put(key, value2);

        // Version must increase after overwrite
        assertTrue(version2 > version1,
                "Version must increase on overwrite. v1=" + version1 + ", v2=" + version2);

        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isPresent());
        assertArrayEquals(value2, entry.get().value(), "Value should be the second write");
    }

    @Test
    @Timeout(60)
    void testDelete() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/todelete";
        byte[] value = "delete-me".getBytes(StandardCharsets.UTF_8);

        kvStore().put(key, value);
        boolean deleted = kvStore().delete(key);
        assertTrue(deleted, "Expected delete to return true for existing key");

        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isEmpty(), "Expected key to be gone after delete");
    }

    @Test
    @Timeout(60)
    void testDeleteNonExistent() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/neverexisted";

        boolean deleted = kvStore().delete(key);
        assertFalse(deleted, "Expected delete to return false for nonexistent key");
    }

    @Test
    @Timeout(60)
    void testDeleteTree() throws KVStoreException {
        String prefix = uniquePrefix();

        // Create multiple keys under the prefix
        kvStore().put(prefix + "/a", "val-a".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/b", "val-b".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/c/deep", "val-deep".getBytes(StandardCharsets.UTF_8));

        kvStore().deleteTree(prefix);

        assertTrue(kvStore().get(prefix + "/a").isEmpty(), "/a should be deleted");
        assertTrue(kvStore().get(prefix + "/b").isEmpty(), "/b should be deleted");
        assertTrue(kvStore().get(prefix + "/c/deep").isEmpty(), "/c/deep should be deleted");
    }

    @Test
    @Timeout(60)
    void testList() throws KVStoreException {
        String prefix = uniquePrefix();

        kvStore().put(prefix + "/child1", "v1".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/child2", "v2".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/child3", "v3".getBytes(StandardCharsets.UTF_8));

        List<KVEntry> children = kvStore().list(prefix);
        assertEquals(3, children.size(), "Expected exactly 3 direct children");
    }

    @Test
    @Timeout(60)
    void testListDirectChildrenOnly() throws KVStoreException {
        String prefix = uniquePrefix();

        // /prefix/b is a direct child; /prefix/b/c is a grandchild
        kvStore().put(prefix + "/b", "val-b".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/b/c", "val-bc".getBytes(StandardCharsets.UTF_8));

        List<KVEntry> children = kvStore().list(prefix);

        // list() should return only direct children -- /prefix/b, not /prefix/b/c
        assertEquals(1, children.size(),
                "Expected only 1 direct child (not grandchild). Got: " + children);
        assertTrue(children.get(0).key().endsWith("/b"),
                "Expected direct child to be /b, got: " + children.get(0).key());
    }

    @Test
    @Timeout(60)
    void testListEmptyPrefix() throws KVStoreException {
        String prefix = uniquePrefix();

        List<KVEntry> children = kvStore().list(prefix);
        assertTrue(children.isEmpty(), "Expected empty list for nonexistent prefix");
    }

    // === CAS Tests ===

    @Test
    @Timeout(60)
    void testCasCreateIfAbsent() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/cas-create";
        byte[] value = "cas-value".getBytes(StandardCharsets.UTF_8);

        // CAS with expectedVersion=0, key does not exist -> should succeed
        long version = kvStore().cas(key, value, 0);
        assertTrue(version >= 0, "CAS create version must be >= 0");

        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isPresent());
        assertArrayEquals(value, entry.get().value());
    }

    @Test
    @Timeout(60)
    void testCasCreateIfAbsentConflict() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/cas-conflict";
        byte[] value = "existing".getBytes(StandardCharsets.UTF_8);

        // Pre-create the key
        kvStore().put(key, value);

        // CAS with expectedVersion=0, key already exists -> VERSION_CONFLICT
        try {
            kvStore().cas(key, "new-value".getBytes(StandardCharsets.UTF_8), 0);
            // If we get here, the backend did not reject the CAS -- that is a bug.
            throw new AssertionError("CAS with expectedVersion=0 should have thrown VERSION_CONFLICT");
        } catch (KVStoreException e) {
            assertEquals(KVStoreException.Code.VERSION_CONFLICT, e.code(),
                    "Expected VERSION_CONFLICT, got: " + e.code());
        }
    }

    @Test
    @Timeout(60)
    void testCasUpdate() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/cas-update";
        byte[] value1 = "original".getBytes(StandardCharsets.UTF_8);
        byte[] value2 = "updated".getBytes(StandardCharsets.UTF_8);

        kvStore().put(key, value1);

        // Read back to get the current version from the entry (backend-agnostic)
        Optional<KVEntry> entry = kvStore().get(key);
        assertTrue(entry.isPresent());
        long currentVersion = entry.get().version();

        // CAS with correct version -> should succeed
        long version2 = kvStore().cas(key, value2, currentVersion);
        assertTrue(version2 > currentVersion,
                "CAS update version must increase. prev=" + currentVersion + ", new=" + version2);

        Optional<KVEntry> updated = kvStore().get(key);
        assertTrue(updated.isPresent());
        assertArrayEquals(value2, updated.get().value());
    }

    @Test
    @Timeout(60)
    void testCasUpdateWrongVersion() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/cas-wrongver";
        byte[] value = "original".getBytes(StandardCharsets.UTF_8);

        kvStore().put(key, value);

        // Use a deliberately wrong version
        long wrongVersion = 999999L;
        try {
            kvStore().cas(key, "should-fail".getBytes(StandardCharsets.UTF_8), wrongVersion);
            throw new AssertionError("CAS with wrong version should have thrown VERSION_CONFLICT");
        } catch (KVStoreException e) {
            assertEquals(KVStoreException.Code.VERSION_CONFLICT, e.code(),
                    "Expected VERSION_CONFLICT, got: " + e.code());
        }
    }

    // === Watch Tests ===

    @Test
    @Timeout(60)
    void testWatchPut() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String key = prefix + "/watched";
        byte[] value = "watch-value".getBytes(StandardCharsets.UTF_8);

        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<KVWatchEvent> receivedEvent = new AtomicReference<>();

        Closeable watch = kvStore().watch(prefix, event -> {
            // Filter for our specific key to avoid noise from parent node creation
            if (event.entry() != null && event.entry().key().equals(key)
                    && event.type() == KVWatchEvent.Type.PUT) {
                receivedEvent.set(event);
                latch.countDown();
            }
        });

        try {
            // Allow watch to initialize
            Thread.sleep(1000);

            kvStore().put(key, value);

            assertTrue(latch.await(30, TimeUnit.SECONDS),
                    "Watch did not receive PUT event within 30 seconds");

            KVWatchEvent event = receivedEvent.get();
            assertNotNull(event, "Event must not be null");
            assertEquals(KVWatchEvent.Type.PUT, event.type());
            assertNotNull(event.entry(), "Entry must not be null for PUT event");
            assertEquals(key, event.entry().key());
            assertArrayEquals(value, event.entry().value());
        } finally {
            watch.close();
        }
    }

    @Test
    @Timeout(60)
    void testWatchDelete() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String key = prefix + "/to-delete-watch";
        byte[] value = "will-be-deleted".getBytes(StandardCharsets.UTF_8);

        // Pre-create the key
        kvStore().put(key, value);

        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<KVWatchEvent> receivedEvent = new AtomicReference<>();

        Closeable watch = kvStore().watch(prefix, event -> {
            if (event.type() == KVWatchEvent.Type.DELETE) {
                receivedEvent.set(event);
                latch.countDown();
            }
        });

        try {
            // Allow watch to initialize
            Thread.sleep(1000);

            kvStore().delete(key);

            assertTrue(latch.await(30, TimeUnit.SECONDS),
                    "Watch did not receive DELETE event within 30 seconds");

            KVWatchEvent event = receivedEvent.get();
            assertNotNull(event, "Event must not be null");
            assertEquals(KVWatchEvent.Type.DELETE, event.type());
        } finally {
            watch.close();
        }
    }

    @Test
    @Timeout(60)
    void testWatchClose() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String key = prefix + "/watch-close";
        byte[] value = "value".getBytes(StandardCharsets.UTF_8);

        AtomicBoolean eventAfterClose = new AtomicBoolean(false);

        Closeable watch = kvStore().watch(prefix, event -> {
            if (event.entry() != null && event.entry().key().equals(key)) {
                eventAfterClose.set(true);
            }
        });

        // Allow watch to initialize
        Thread.sleep(1000);

        // Close the watch BEFORE writing
        watch.close();

        // Allow close to propagate
        Thread.sleep(1000);

        // Write after the watch is closed
        kvStore().put(key, value);

        // Wait a reasonable time and verify no event was received
        Thread.sleep(3000);

        assertFalse(eventAfterClose.get(),
                "Should not receive events after watch is closed");
    }

    // === Lock Tests ===

    @Test
    @Timeout(60)
    void testAcquireAndReleaseLock() throws KVStoreException, IOException {
        String prefix = uniquePrefix();
        String lockPath = prefix + "/lock";

        Closeable lock = kvStore().acquireLock(lockPath);
        assertNotNull(lock, "Lock handle must not be null");

        // Release
        lock.close();

        // Should be able to re-acquire after release
        Closeable lock2 = kvStore().acquireLock(lockPath);
        assertNotNull(lock2);
        lock2.close();
    }

    @Test
    @Timeout(60)
    void testLockMutualExclusion() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String lockPath = prefix + "/mutex";

        // Thread 1 acquires the lock
        Closeable lock1 = kvStore().acquireLock(lockPath);

        CountDownLatch thread2Started = new CountDownLatch(1);
        CountDownLatch thread2Acquired = new CountDownLatch(1);
        AtomicReference<Closeable> lock2Ref = new AtomicReference<>();
        AtomicReference<Throwable> error = new AtomicReference<>();

        Thread thread2 = new Thread(() -> {
            try {
                thread2Started.countDown();
                // This should block until lock1 is released
                Closeable lock2 = kvStore().acquireLock(lockPath);
                lock2Ref.set(lock2);
                thread2Acquired.countDown();
            } catch (Throwable t) {
                error.set(t);
                thread2Acquired.countDown();
            }
        }, "lock-contender");
        thread2.setDaemon(true);
        thread2.start();

        // Wait for thread 2 to start trying to acquire
        assertTrue(thread2Started.await(5, TimeUnit.SECONDS));

        // Give thread 2 a moment to block on lock acquisition
        Thread.sleep(2000);

        // Thread 2 should NOT have acquired the lock yet
        assertEquals(1, thread2Acquired.getCount(),
                "Thread 2 should be blocked waiting for the lock");

        // Release lock1
        lock1.close();

        // Thread 2 should now acquire the lock
        assertTrue(thread2Acquired.await(30, TimeUnit.SECONDS),
                "Thread 2 should have acquired the lock after thread 1 released it");

        if (error.get() != null) {
            throw new AssertionError("Thread 2 encountered an error", error.get());
        }

        // Clean up lock2
        Closeable lock2 = lock2Ref.get();
        if (lock2 != null) {
            lock2.close();
        }
    }

    // === Leader Election Tests ===

    @Test
    @Timeout(60)
    void testLeaderElectionSingleParticipant() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String electionPath = prefix + "/election-single";

        KVStore.LeaderElection election = kvStore().leaderElection(electionPath, "participant-1");

        try {
            // Wait for election to converge
            boolean becameLeader = waitForCondition(() -> election.isLeader(), 30_000);
            assertTrue(becameLeader,
                    "Single participant should become leader within 30 seconds");
        } finally {
            election.close();
        }
    }

    @Test
    @Timeout(60)
    void testLeaderElectionMultipleParticipants() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String electionPath = prefix + "/election-multi";

        KVStore.LeaderElection election1 = kvStore().leaderElection(electionPath, "participant-A");
        KVStore.LeaderElection election2 = kvStore().leaderElection(electionPath, "participant-B");

        try {
            // Wait for at least one to become leader
            boolean anyLeader = waitForCondition(
                    () -> election1.isLeader() || election2.isLeader(), 30_000);
            assertTrue(anyLeader, "At least one participant should become leader");

            // Exactly one should be leader (not both)
            // Note: for etcd, campaign() blocks until leadership, so election2 may still
            // be blocking. We check that at least one is leader and at most one is.
            if (election1.isLeader() && election2.isLeader()) {
                // Both claim leadership -- this is acceptable briefly during transitions
                // but should not persist. Let it settle.
                Thread.sleep(2000);
                // After settling, at most one should be leader
                assertFalse(election1.isLeader() && election2.isLeader(),
                        "Both participants should not hold leadership simultaneously after settling");
            }

            // Close the current leader and verify the other takes over
            KVStore.LeaderElection leader = election1.isLeader() ? election1 : election2;
            KVStore.LeaderElection follower = election1.isLeader() ? election2 : election1;

            leader.close();

            // Wait for the follower to become leader
            boolean followerBecameLeader = waitForCondition(follower::isLeader, 30_000);
            assertTrue(followerBecameLeader,
                    "Follower should become leader after original leader closes");
        } finally {
            // Clean up -- close() is idempotent
            election1.close();
            election2.close();
        }
    }

    @Test
    @Timeout(60)
    void testLeaderElectionListener() throws KVStoreException, InterruptedException, IOException {
        String prefix = uniquePrefix();
        String electionPath = prefix + "/election-listener";

        CountDownLatch leaderLatch = new CountDownLatch(1);
        AtomicBoolean leadershipGranted = new AtomicBoolean(false);

        KVStore.LeaderElection election = kvStore().leaderElection(electionPath, "listener-test");

        election.addListener(isLeader -> {
            if (isLeader) {
                leadershipGranted.set(true);
                leaderLatch.countDown();
            }
        });

        try {
            // Wait for listener notification or isLeader() to become true
            // Note: for some backends the listener fires before addListener returns
            // if the node is already the leader
            boolean notified = leaderLatch.await(30, TimeUnit.SECONDS);

            // The listener may have been called, or leadership may already be true
            assertTrue(notified || election.isLeader(),
                    "Listener should have been called with isLeader=true, or isLeader() should return true");
        } finally {
            election.close();
        }
    }

    // === Health Check Tests ===

    @Test
    @Timeout(60)
    void testBackendHealthCheck() throws KVStoreException {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("dummy"))
                .startupHealthCheckRetries(1)
                .maxAcceptableLatencyMs(10_000);

        // This should pass without exception
        BackendHealthChecker.check(kvStore(), config);
    }

    // === Helper methods ===

    /**
     * Polls a condition with a timeout. Returns true if the condition became true
     * within the timeout period, false otherwise.
     */
    private static boolean waitForCondition(BooleanSupplier condition, long timeoutMs)
            throws InterruptedException {
        long deadline = System.currentTimeMillis() + timeoutMs;
        while (System.currentTimeMillis() < deadline) {
            if (condition.getAsBoolean()) {
                return true;
            }
            Thread.sleep(200);
        }
        return condition.getAsBoolean();
    }

    @FunctionalInterface
    private interface BooleanSupplier {
        boolean getAsBoolean();
    }

    // === Regression tests for production fixes ===

    @Test
    @Timeout(60)
    void testDeleteTreeDoesNotDeleteSiblings() throws KVStoreException {
        String prefix = uniquePrefix();
        kvStore().put(prefix + "/target", "val".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/target/child", "val".getBytes(StandardCharsets.UTF_8));
        kvStore().put(prefix + "/target-sibling", "should-survive".getBytes(StandardCharsets.UTF_8));

        kvStore().deleteTree(prefix + "/target");

        assertTrue(kvStore().get(prefix + "/target").isEmpty(), "Target key should be deleted");
        assertTrue(kvStore().get(prefix + "/target/child").isEmpty(), "Child key should be deleted");
        assertFalse(kvStore().get(prefix + "/target-sibling").isEmpty(),
                "Sibling key must NOT be deleted by deleteTree");
    }

    @Test
    @Timeout(60)
    void testCreateVersionStableAfterOverwrite() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/cv-test";

        kvStore().put(key, "v1".getBytes(StandardCharsets.UTF_8));
        KVEntry firstRead = kvStore().get(key).orElseThrow();
        long createVersion = firstRead.createVersion();

        // Overwrite the same key
        kvStore().put(key, "v2".getBytes(StandardCharsets.UTF_8));
        KVEntry secondRead = kvStore().get(key).orElseThrow();

        assertEquals(createVersion, secondRead.createVersion(),
                "createVersion must remain stable after overwrite");
        assertTrue(secondRead.version() > firstRead.version(),
                "version must increase after overwrite");
    }

    @Test
    @Timeout(60)
    void testPutAndGetBinaryData() throws KVStoreException {
        String prefix = uniquePrefix();
        String key = prefix + "/binary";

        // Include bytes that are invalid UTF-8: 0xFF, 0xFE, embedded nulls
        byte[] binaryData = new byte[]{0x00, 0x01, (byte) 0xFF, (byte) 0xFE, 0x7F, 0x00, (byte) 0x80};
        kvStore().put(key, binaryData);

        KVEntry entry = kvStore().get(key).orElseThrow();
        assertArrayEquals(binaryData, entry.value(),
                "Binary data must survive a put/get round-trip without corruption");
    }
}
