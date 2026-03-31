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
package com.shieldblaze.expressgateway.controlplane.config;

import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
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
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ChangeJournalTest {

    private static final String BASE_PATH = "/controlplane/journal";

    private InMemoryKVStore kvStore;
    private ChangeJournal journal;

    @BeforeEach
    void setUp() throws KVStoreException {
        kvStore = new InMemoryKVStore();
        journal = new ChangeJournal(kvStore, BASE_PATH);
    }

    // ----------------------------------------------------------------
    // Helper: create a ConfigMutation.Upsert with a unique name.
    // Uses a real ConfigSpec subtype (ClusterSpec) to exercise the full
    // Jackson polymorphic serialization/deserialization path (HIGH-5 fix).
    // ----------------------------------------------------------------

    private static ConfigMutation upsertMutation(String name) {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", name);
        ConfigResource resource = new ConfigResource(
                id,
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                null,
                new ClusterSpec(name, "round-robin", "default-hc", 100, 30)
        );
        return new ConfigMutation.Upsert(resource);
    }

    private static ConfigMutation deleteMutation(String name) {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", name);
        return new ConfigMutation.Delete(id);
    }

    // ----------------------------------------------------------------
    // 1. Initial state
    // ----------------------------------------------------------------

    @Test
    void initialStateHasRevisionZero() {
        assertEquals(0, journal.currentRevision());
    }

    @Test
    void initialStateEntriesSinceZeroIsEmpty() throws KVStoreException {
        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertNotNull(entries);
        assertTrue(entries.isEmpty());
    }

    // ----------------------------------------------------------------
    // 2. Append and retrieve
    // ----------------------------------------------------------------

    @Test
    void appendSingleMutationIncrementsRevisionAndIsRetrievable() throws KVStoreException {
        ConfigMutation mutation = upsertMutation("prod-1");
        long rev = journal.append(List.of(mutation));

        assertEquals(1, rev);
        assertEquals(1, journal.currentRevision());

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertEquals(1, entries.size());

        ChangeJournal.JournalEntry entry = entries.getFirst();
        assertEquals(1, entry.globalRevision());
        assertNotNull(entry.timestamp());
        assertEquals(1, entry.mutations().size());
    }

    @Test
    void appendMultipleMutationsInSingleBatch() throws KVStoreException {
        List<ConfigMutation> batch = List.of(
                upsertMutation("prod-1"),
                upsertMutation("prod-2"),
                deleteMutation("staging-1")
        );

        long rev = journal.append(batch);

        assertEquals(1, rev);
        assertEquals(1, journal.currentRevision());

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertEquals(1, entries.size());
        assertEquals(3, entries.getFirst().mutations().size());
    }

    // ----------------------------------------------------------------
    // 3. Multiple appends - sequential revisions
    // ----------------------------------------------------------------

    @Test
    void multipleAppendsProduceSequentialRevisions() throws KVStoreException {
        long rev1 = journal.append(List.of(upsertMutation("prod-1")));
        long rev2 = journal.append(List.of(upsertMutation("prod-2")));
        long rev3 = journal.append(List.of(upsertMutation("prod-3")));

        assertEquals(1, rev1);
        assertEquals(2, rev2);
        assertEquals(3, rev3);
        assertEquals(3, journal.currentRevision());

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertEquals(3, entries.size());
        assertEquals(1, entries.get(0).globalRevision());
        assertEquals(2, entries.get(1).globalRevision());
        assertEquals(3, entries.get(2).globalRevision());
    }

    // ----------------------------------------------------------------
    // 4. entriesSince filters correctly
    // ----------------------------------------------------------------

    @Test
    void entriesSinceFiltersCorrectly() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        // fromRevision=1 should return entries 2 and 3 only
        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(1);
        assertEquals(2, entries.size());
        assertEquals(2, entries.get(0).globalRevision());
        assertEquals(3, entries.get(1).globalRevision());
    }

    @Test
    void entriesSinceCurrentRevisionReturnsEmpty() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(2);
        assertTrue(entries.isEmpty());
    }

    @Test
    void entriesSinceBeyondCurrentRevisionReturnsEmpty() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(100);
        assertTrue(entries.isEmpty());
    }

    @Test
    void entriesSinceRejectsNegativeRevision() {
        assertThrows(IllegalArgumentException.class, () -> journal.entriesSince(-1));
    }

    // ----------------------------------------------------------------
    // 5. Empty mutations throws
    // ----------------------------------------------------------------

    @Test
    void appendEmptyMutationsThrowsIllegalArgument() {
        assertThrows(IllegalArgumentException.class, () -> journal.append(List.of()));
    }

    // ----------------------------------------------------------------
    // 6. Null mutations throws
    // ----------------------------------------------------------------

    @Test
    void appendNullMutationsThrowsNullPointer() {
        assertThrows(NullPointerException.class, () -> journal.append(null));
    }

    // ----------------------------------------------------------------
    // 7. Compact removes old entries
    // ----------------------------------------------------------------

    @Test
    void compactRemovesEntriesUpToRevision() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        journal.compact(2);

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertEquals(1, entries.size());
        assertEquals(3, entries.getFirst().globalRevision());
    }

    @Test
    void compactDoesNotAffectCurrentRevision() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        journal.compact(2);

        // currentRevision should remain 3, not be reset
        assertEquals(3, journal.currentRevision());
    }

    @Test
    void compactAllEntriesLeavesJournalEmpty() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));

        journal.compact(2);

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertTrue(entries.isEmpty());
        assertEquals(2, journal.currentRevision());
    }

    // ----------------------------------------------------------------
    // 8. Compact with invalid revision
    // ----------------------------------------------------------------

    @Test
    void compactWithZeroRevisionThrowsIllegalArgument() {
        assertThrows(IllegalArgumentException.class, () -> journal.compact(0));
    }

    @Test
    void compactWithNegativeRevisionThrowsIllegalArgument() {
        assertThrows(IllegalArgumentException.class, () -> journal.compact(-1));
    }

    // ----------------------------------------------------------------
    // 9. Recovery from KV store
    // ----------------------------------------------------------------

    @Test
    void newJournalRecoverEntriesFromKVStore() throws KVStoreException {
        // Append entries to the first journal instance
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        // Create a new journal backed by the same KV store
        ChangeJournal recoveredJournal = new ChangeJournal(kvStore, BASE_PATH);

        assertEquals(3, recoveredJournal.currentRevision());

        List<ChangeJournal.JournalEntry> entries = recoveredJournal.entriesSince(0);
        assertEquals(3, entries.size());
        assertEquals(1, entries.get(0).globalRevision());
        assertEquals(2, entries.get(1).globalRevision());
        assertEquals(3, entries.get(2).globalRevision());
    }

    @Test
    void recoveredJournalContinuesRevisionSequence() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));

        // Recover
        ChangeJournal recoveredJournal = new ChangeJournal(kvStore, BASE_PATH);
        long rev = recoveredJournal.append(List.of(upsertMutation("prod-3")));

        assertEquals(3, rev);
        assertEquals(3, recoveredJournal.currentRevision());
    }

    @Test
    void recoveredJournalAfterCompactionOnlySeesRemainingEntries() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        journal.compact(2);

        ChangeJournal recoveredJournal = new ChangeJournal(kvStore, BASE_PATH);

        // Only entry 3 should remain; revision should recover to 3
        assertEquals(3, recoveredJournal.currentRevision());

        List<ChangeJournal.JournalEntry> entries = recoveredJournal.entriesSince(0);
        assertEquals(1, entries.size());
        assertEquals(3, entries.getFirst().globalRevision());
    }

    // ----------------------------------------------------------------
    // Additional edge cases
    // ----------------------------------------------------------------

    @Test
    void entriesAreSortedByRevision() throws KVStoreException {
        journal.append(List.of(upsertMutation("a")));
        journal.append(List.of(upsertMutation("b")));
        journal.append(List.of(upsertMutation("c")));
        journal.append(List.of(upsertMutation("d")));
        journal.append(List.of(upsertMutation("e")));

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        for (int i = 1; i < entries.size(); i++) {
            assertTrue(entries.get(i).globalRevision() > entries.get(i - 1).globalRevision(),
                    "Entries must be sorted by ascending revision");
        }
    }

    @Test
    void entriesSinceReturnsUnmodifiableList() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));

        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        assertThrows(UnsupportedOperationException.class, () -> entries.add(null));
    }

    @Test
    void journalEntryMutationsListIsImmutable() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));

        ChangeJournal.JournalEntry entry = journal.entriesSince(0).getFirst();
        assertThrows(UnsupportedOperationException.class, () -> entry.mutations().add(null));
    }

    @Test
    void appendWritesToKVStore() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));

        // The KV store should have at least one entry under the base path
        List<KVEntry> kvEntries = kvStore.list(BASE_PATH);
        assertEquals(1, kvEntries.size());
        assertTrue(kvEntries.getFirst().key().startsWith(BASE_PATH + "/"));
    }

    @Test
    void compactRemovesFromKVStore() throws KVStoreException {
        journal.append(List.of(upsertMutation("prod-1")));
        journal.append(List.of(upsertMutation("prod-2")));
        journal.append(List.of(upsertMutation("prod-3")));

        journal.compact(2);

        List<KVEntry> kvEntries = kvStore.list(BASE_PATH);
        assertEquals(1, kvEntries.size());
    }

    // ----------------------------------------------------------------
    // InMemoryKVStore implementation for testing
    // ----------------------------------------------------------------

    private static class InMemoryKVStore implements KVStore {
        private final ConcurrentHashMap<String, byte[]> store = new ConcurrentHashMap<>();
        private final AtomicLong version = new AtomicLong(0);

        @Override
        public Optional<KVEntry> get(String key) throws KVStoreException {
            byte[] v = store.get(key);
            return v == null ? Optional.empty() : Optional.of(new KVEntry(key, v, version.get(), 1));
        }

        @Override
        public List<KVEntry> list(String prefix) throws KVStoreException {
            return store.entrySet().stream()
                    .filter(e -> e.getKey().startsWith(prefix + "/"))
                    .sorted(Map.Entry.comparingByKey())
                    .map(e -> new KVEntry(e.getKey(), e.getValue(), version.get(), 1))
                    .toList();
        }

        @Override
        public List<KVEntry> listRecursive(String prefix) throws KVStoreException {
            return list(prefix);
        }

        @Override
        public long put(String key, byte[] value) throws KVStoreException {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public long cas(String key, byte[] value, long expectedVersion) throws KVStoreException {
            store.put(key, value);
            return version.incrementAndGet();
        }

        @Override
        public boolean delete(String key) throws KVStoreException {
            return store.remove(key) != null;
        }

        @Override
        public void deleteTree(String key) throws KVStoreException {
            store.keySet().removeIf(k -> k.startsWith(key));
        }

        @Override
        public Closeable watch(String keyOrPrefix, KVWatcher watcher) throws KVStoreException {
            return () -> {
            };
        }

        @Override
        public Closeable acquireLock(String lockPath) throws KVStoreException {
            return () -> {
            };
        }

        @Override
        public LeaderElection leaderElection(String electionPath, String participantId) throws KVStoreException {
            return new LeaderElection() {
                @Override
                public boolean isLeader() {
                    return true;
                }

                @Override
                public String currentLeaderId() {
                    return participantId;
                }

                @Override
                public void addListener(LeaderChangeListener listener) {
                }

                @Override
                public void close() {
                }
            };
        }

        @Override
        public void close() {
        }
    }
}
