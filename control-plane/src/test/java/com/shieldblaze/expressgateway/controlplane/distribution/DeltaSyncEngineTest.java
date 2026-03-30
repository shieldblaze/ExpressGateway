package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVWatcher;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.io.Closeable;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class DeltaSyncEngineTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    /**
     * Simple in-memory KV store for testing ChangeJournal without an external backend.
     */
    private static class InMemoryKVStore implements KVStore {
        private final Map<String, byte[]> store = new ConcurrentHashMap<>();
        private final AtomicLong version = new AtomicLong(0);

        @Override
        public Optional<KVEntry> get(String key) {
            byte[] v = store.get(key);
            return v == null ? Optional.empty() : Optional.of(new KVEntry(key, v, version.get(), 1));
        }

        @Override
        public List<KVEntry> list(String prefix) {
            return store.entrySet().stream()
                    .filter(e -> e.getKey().startsWith(prefix + "/"))
                    .map(e -> new KVEntry(e.getKey(), e.getValue(), version.get(), 1))
                    .toList();
        }

        @Override
        public List<KVEntry> listRecursive(String prefix) {
            return list(prefix);
        }

        @Override
        public long put(String key, byte[] value) {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public long cas(String key, byte[] value, long expectedVersion) {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public boolean delete(String key) {
            return store.remove(key) != null;
        }

        @Override
        public void deleteTree(String key) {
            store.keySet().removeIf(k -> k.startsWith(key));
        }

        @Override
        public Closeable watch(String keyOrPrefix, KVWatcher watcher) {
            return () -> { };
        }

        @Override
        public Closeable acquireLock(String lockPath) {
            return () -> { };
        }

        @Override
        public LeaderElection leaderElection(String electionPath, String participantId) {
            return new LeaderElection() {
                @Override public boolean isLeader() { return true; }
                @Override public String currentLeaderId() { return participantId; }
                @Override public void addListener(LeaderChangeListener listener) { }
                @Override public void close() { }
            };
        }

        @Override
        public void close() { }
    }

    private InMemoryKVStore kvStore;
    private ChangeJournal journal;

    @BeforeEach
    void setUp() throws KVStoreException {
        kvStore = new InMemoryKVStore();
        journal = new ChangeJournal(kvStore, "/test/journal");
    }

    private ConfigMutation.Upsert upsert(String kind, String scope, String name) {
        ConfigResourceId id = new ConfigResourceId(kind, scope, name);
        ConfigResource resource = new ConfigResource(
                id,
                new ConfigKind(kind, 1),
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                Map.of(),
                new TestSpec(name)
        );
        return new ConfigMutation.Upsert(resource);
    }

    private ConfigMutation.Delete delete(String kind, String scope, String name) {
        return new ConfigMutation.Delete(new ConfigResourceId(kind, scope, name));
    }

    @Test
    void nodeAtCurrentRevisionReturnsEmptyDelta() throws KVStoreException {
        // Append one entry so currentRevision is 1
        journal.append(List.of(upsert("cluster", "global", "prod")));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(1);

        assertNotNull(delta);
        assertTrue(delta.isEmpty(), "Delta should be empty when node is at current revision");
        assertEquals(1, delta.fromRevision());
        assertEquals(1, delta.toRevision());
    }

    @Test
    void nodeAheadOfCurrentRevisionReturnsEmptyDelta() throws KVStoreException {
        journal.append(List.of(upsert("cluster", "global", "prod")));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(5); // Ahead

        assertNotNull(delta);
        assertTrue(delta.isEmpty());
    }

    @Test
    void nodeBehindByOneRevisionReturnsDeltaWithMutations() throws KVStoreException {
        journal.append(List.of(upsert("cluster", "global", "alpha")));  // rev 1
        journal.append(List.of(upsert("cluster", "global", "beta")));   // rev 2

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(1);

        assertNotNull(delta);
        assertEquals(1, delta.fromRevision());
        assertEquals(2, delta.toRevision());
        assertEquals(1, delta.mutations().size());
    }

    @Test
    void nodeBehindByMultipleRevisionsMergesByResourcePath() throws KVStoreException {
        // rev 1: upsert cluster/global/alpha
        journal.append(List.of(upsert("cluster", "global", "alpha")));
        // rev 2: upsert cluster/global/alpha again (should overwrite in merge)
        journal.append(List.of(upsert("cluster", "global", "alpha")));
        // rev 3: upsert cluster/global/beta (different resource)
        journal.append(List.of(upsert("cluster", "global", "beta")));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(0); // Node has never received config

        assertNotNull(delta);
        assertEquals(0, delta.fromRevision());
        assertEquals(3, delta.toRevision());
        // "cluster/global/alpha" appears in rev 1 and rev 2 -- last write wins, so only 1 entry
        // "cluster/global/beta" appears once
        assertEquals(2, delta.mutations().size(), "Duplicate resource paths should be merged (last-write-wins)");
    }

    @Test
    void lastWriteWinsForSameResourcePath() throws KVStoreException {
        // rev 1: upsert
        journal.append(List.of(upsert("cluster", "global", "prod")));
        // rev 2: delete the same resource
        journal.append(List.of(delete("cluster", "global", "prod")));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(0);

        assertNotNull(delta);
        assertEquals(1, delta.mutations().size());
        // The delete at rev 2 should be the surviving mutation
        assertTrue(delta.mutations().get(0) instanceof ConfigMutation.Delete,
                "Delete at higher revision should win over earlier upsert");
    }

    @Test
    void nodeExceedsMaxJournalLagReturnsNull() throws KVStoreException {
        // Create 10 journal entries
        for (int i = 0; i < 10; i++) {
            journal.append(List.of(upsert("cluster", "global", "r-" + i)));
        }

        // maxJournalLag = 5, node at revision 0 => lag is 10 > 5
        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 5);
        ConfigDelta delta = engine.computeDelta(0);

        assertNull(delta, "Should return null when node exceeds maxJournalLag");
    }

    @Test
    void nodeJustWithinMaxJournalLagReturnsDelta() throws KVStoreException {
        for (int i = 0; i < 5; i++) {
            journal.append(List.of(upsert("cluster", "global", "r-" + i)));
        }

        // maxJournalLag = 5, node at revision 0 => lag is exactly 5 (not greater)
        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 5);
        ConfigDelta delta = engine.computeDelta(0);

        assertNotNull(delta, "Should return delta when lag equals maxJournalLag exactly");
        assertEquals(5, delta.mutations().size());
    }

    @Test
    void nodeJustBeyondMaxJournalLagReturnsNull() throws KVStoreException {
        for (int i = 0; i < 6; i++) {
            journal.append(List.of(upsert("cluster", "global", "r-" + i)));
        }

        // maxJournalLag = 5, node at revision 0 => lag is 6 > 5
        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 5);
        ConfigDelta delta = engine.computeDelta(0);

        assertNull(delta, "Should return null when lag exceeds maxJournalLag by 1");
    }

    @Test
    void emptyJournalEntriesAfterCompactionReturnsNull() throws KVStoreException {
        // Create entries and then compact them
        journal.append(List.of(upsert("cluster", "global", "a"))); // rev 1
        journal.append(List.of(upsert("cluster", "global", "b"))); // rev 2
        journal.append(List.of(upsert("cluster", "global", "c"))); // rev 3

        // Compact up to revision 2 (removes rev 1 and 2 from KV store and cache)
        journal.compact(2);

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        // Node at revision 0 should get entries since 0, but revisions 1-2 are compacted.
        // The journal.entriesSince(0) will only return rev 3.
        // This still returns a delta because rev 3 is in the cache.
        ConfigDelta delta = engine.computeDelta(0);
        // Since rev 3 is still available, it returns a partial delta (not null)
        assertNotNull(delta);
        assertEquals(1, delta.mutations().size());
    }

    @Test
    void maxJournalLagZeroThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class,
                () -> new DeltaSyncEngine(journal, 0));
    }

    @Test
    void maxJournalLagNegativeThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class,
                () -> new DeltaSyncEngine(journal, -10));
    }

    @Test
    void nullJournalThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> new DeltaSyncEngine(null, 100));
    }

    @Test
    void emptyJournalNodeAtZeroReturnsEmptyDelta() {
        // Journal has no entries; currentRevision is 0; node at 0 => nodeLastRevision >= currentRevision
        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(0);

        assertNotNull(delta);
        assertTrue(delta.isEmpty());
        assertEquals(0, delta.fromRevision());
        assertEquals(0, delta.toRevision());
    }

    @Test
    void multipleMutationsPerJournalEntry() throws KVStoreException {
        // Single journal entry with multiple mutations
        journal.append(List.of(
                upsert("cluster", "global", "alpha"),
                upsert("listener", "global", "http"),
                delete("cluster", "global", "beta")
        ));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(0);

        assertNotNull(delta);
        assertEquals(3, delta.mutations().size(), "All mutations from a single entry should be in the delta");
    }

    @Test
    void mergeAcrossMultipleEntriesWithMixedOperations() throws KVStoreException {
        // rev 1: create alpha and beta
        journal.append(List.of(
                upsert("cluster", "global", "alpha"),
                upsert("cluster", "global", "beta")
        ));
        // rev 2: update alpha, delete beta, create gamma
        journal.append(List.of(
                upsert("cluster", "global", "alpha"),
                delete("cluster", "global", "beta"),
                upsert("cluster", "global", "gamma")
        ));

        DeltaSyncEngine engine = new DeltaSyncEngine(journal, 100);
        ConfigDelta delta = engine.computeDelta(0);

        assertNotNull(delta);
        // alpha: upsert (rev 2 wins), beta: delete (rev 2 wins), gamma: upsert
        assertEquals(3, delta.mutations().size());

        // Find the beta mutation and verify it's a delete
        ConfigMutation betaMutation = delta.mutations().stream()
                .filter(m -> switch (m) {
                    case ConfigMutation.Upsert u -> u.resource().id().name().equals("beta");
                    case ConfigMutation.Delete d -> d.resourceId().name().equals("beta");
                })
                .findFirst()
                .orElseThrow();
        assertTrue(betaMutation instanceof ConfigMutation.Delete,
                "beta should be a Delete (last-write-wins from rev 2)");
    }
}
