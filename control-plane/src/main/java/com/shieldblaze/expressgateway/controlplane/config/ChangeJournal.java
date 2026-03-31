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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.databind.ObjectMapper;
import com.fasterxml.jackson.datatype.jsr310.JavaTimeModule;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVEntry;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.IOException;
import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.ConcurrentNavigableMap;
import java.util.concurrent.ConcurrentSkipListMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Records configuration changes with a monotonically increasing global revision
 * to support delta synchronization between control plane and data plane nodes.
 *
 * <p>Journal entries are stored as sequential keys under the configured base path
 * in the KV store. Each entry contains the global revision, timestamp, and the
 * list of mutations that were applied.</p>
 *
 * <p>Typical usage flow:</p>
 * <ol>
 *   <li>Control plane applies a {@link ConfigTransaction}</li>
 *   <li>{@link #append(List)} records the mutations and returns the new revision</li>
 *   <li>Data plane nodes call {@link #entriesSince(long)} to fetch only changes since their last known revision</li>
 * </ol>
 */
public final class ChangeJournal {

    private static final Logger logger = LogManager.getLogger(ChangeJournal.class);
    private static final ObjectMapper MAPPER = new ObjectMapper();

    static {
        MAPPER.registerModule(new JavaTimeModule());
        MAPPER.findAndRegisterModules();
    }

    private final KVStore kvStore;
    private final String basePath;
    private final AtomicLong currentRevision;

    /**
     * In-memory cache of journal entries keyed by revision. Populated on append
     * and on startup from KV store. {@link #entriesSince(long)} reads from this
     * cache to avoid O(N) KV store reads on every delta sync call.
     */
    private final ConcurrentSkipListMap<Long, JournalEntry> cache = new ConcurrentSkipListMap<>();

    /**
     * A single journal entry capturing a set of mutations at a specific global revision.
     *
     * @param globalRevision The monotonically increasing revision number
     * @param timestamp      When the mutations were recorded
     * @param mutations      The list of mutations in this entry
     */
    public record JournalEntry(
            @JsonProperty("globalRevision") long globalRevision,
            @JsonProperty("timestamp") Instant timestamp,
            @JsonProperty("mutations") List<ConfigMutation> mutations
    ) {
        public JournalEntry {
            if (globalRevision < 1) {
                throw new IllegalArgumentException("globalRevision must be >= 1, got: " + globalRevision);
            }
            Objects.requireNonNull(timestamp, "timestamp");
            Objects.requireNonNull(mutations, "mutations");
            mutations = List.copyOf(mutations);
        }
    }

    /**
     * Create a new ChangeJournal backed by the given KV store.
     *
     * @param kvStore  The KV store to persist journal entries
     * @param basePath The base path under which journal entries are stored
     *                 (e.g. "/controlplane/journal")
     * @throws KVStoreException If reading the current revision from the store fails
     */
    public ChangeJournal(KVStore kvStore, String basePath) throws KVStoreException {
        this.kvStore = Objects.requireNonNull(kvStore, "kvStore");
        this.basePath = Objects.requireNonNull(basePath, "basePath");

        // Initialize current revision and in-memory cache from existing entries
        long maxRevision = 0;
        List<KVEntry> existing = kvStore.list(basePath);
        for (KVEntry entry : existing) {
            String key = entry.key();
            String revisionStr = key.substring(key.lastIndexOf('/') + 1);
            try {
                long rev = Long.parseLong(revisionStr);
                if (rev > maxRevision) {
                    maxRevision = rev;
                }
                // Populate cache from KV store for startup/recovery
                JournalEntry journalEntry = MAPPER.readValue(entry.value(), JournalEntry.class);
                cache.put(rev, journalEntry);
            } catch (NumberFormatException e) {
                logger.warn("Skipping non-numeric journal key: {}", key);
            } catch (IOException e) {
                logger.error("Failed to deserialize journal entry at key={} during startup", key, e);
            }
        }
        this.currentRevision = new AtomicLong(maxRevision);
        logger.info("ChangeJournal initialized at basePath={}, currentRevision={}, cachedEntries={}",
                basePath, maxRevision, cache.size());
    }

    /**
     * Append a set of mutations to the journal, assigning the next global revision.
     *
     * <p>Synchronized because the method is called once per batch window (~500ms)
     * so contention is negligible, and it eliminates race conditions between
     * concurrent callers that could cause revision gaps on partial failure.</p>
     *
     * <p>Serialization is performed <em>before</em> incrementing the revision counter.
     * This ensures that neither {@link JsonProcessingException} nor
     * {@link KVStoreException} can leave a permanent gap in the revision sequence.</p>
     *
     * @param mutations The mutations to record (must not be empty)
     * @return The new global revision assigned to this entry
     * @throws KVStoreException If the write to the KV store fails
     */
    public synchronized long append(List<ConfigMutation> mutations) throws KVStoreException {
        Objects.requireNonNull(mutations, "mutations");
        if (mutations.isEmpty()) {
            throw new IllegalArgumentException("mutations must not be empty");
        }

        // Peek at next revision without committing it yet
        long newRevision = currentRevision.get() + 1;
        Instant now = Instant.now();
        JournalEntry entry = new JournalEntry(newRevision, now, mutations);

        // Serialize before incrementing -- if this fails, no revision is consumed
        byte[] serialized;
        try {
            serialized = MAPPER.writeValueAsBytes(entry);
        } catch (JsonProcessingException e) {
            throw new RuntimeException("Failed to serialize journal entry", e);
        }

        // Write to KV store -- if this fails, no revision is consumed
        String key = entryKey(newRevision);
        kvStore.put(key, serialized);

        // Only now commit the revision and populate the cache
        currentRevision.set(newRevision);
        cache.put(newRevision, entry);

        logger.debug("Appended journal entry: revision={}, mutations={}", newRevision, mutations.size());
        return newRevision;
    }

    /**
     * Retrieve all journal entries with a revision strictly greater than {@code fromRevision}.
     *
     * @param fromRevision The exclusive lower bound revision (0 to get all entries)
     * @return A list of journal entries ordered by ascending revision
     * @throws KVStoreException If reading from the KV store fails
     */
    public List<JournalEntry> entriesSince(long fromRevision) throws KVStoreException {
        if (fromRevision < 0) {
            throw new IllegalArgumentException("fromRevision must be >= 0, got: " + fromRevision);
        }

        // Use fromRevision + 1 as the inclusive lower bound (tailMap is inclusive).
        // ConcurrentSkipListMap.tailMap is O(log N) and returns a view -- no copy needed.
        ConcurrentNavigableMap<Long, JournalEntry> tail = cache.tailMap(fromRevision + 1, true);

        if (!tail.isEmpty()) {
            // Cache hit: return entries directly (already sorted by revision key)
            return Collections.unmodifiableList(new ArrayList<>(tail.values()));
        }

        // Cache miss (startup recovery or post-compaction). Fall back to KV store
        // and repopulate the cache so subsequent calls are fast.
        List<KVEntry> entries = kvStore.list(basePath);
        List<JournalEntry> result = new ArrayList<>();

        for (KVEntry kvEntry : entries) {
            try {
                JournalEntry journalEntry = MAPPER.readValue(kvEntry.value(), JournalEntry.class);
                cache.putIfAbsent(journalEntry.globalRevision(), journalEntry);
                if (journalEntry.globalRevision() > fromRevision) {
                    result.add(journalEntry);
                }
            } catch (IOException e) {
                String key = kvEntry.key();
                logger.error("Failed to deserialize journal entry at key={}", key, e);
            }
        }

        result.sort(Comparator.comparingLong(JournalEntry::globalRevision));
        return Collections.unmodifiableList(result);
    }

    /**
     * Returns the current (latest) global revision.
     *
     * @return The current revision, or 0 if no entries exist
     */
    public long currentRevision() {
        return currentRevision.get();
    }

    /**
     * Remove journal entries with revision less than or equal to {@code upToRevision}.
     * Used to prevent unbounded journal growth after all nodes have confirmed
     * they are caught up beyond this revision.
     *
     * <p>Synchronized with {@link #append(List)} to prevent reading partially-deleted
     * entries during concurrent append+compact.</p>
     *
     * @param upToRevision The inclusive upper bound of entries to remove
     * @throws KVStoreException If deleting from the KV store fails
     */
    public synchronized void compact(long upToRevision) throws KVStoreException {
        if (upToRevision < 1) {
            throw new IllegalArgumentException("upToRevision must be >= 1, got: " + upToRevision);
        }

        List<KVEntry> entries = kvStore.list(basePath);
        int removed = 0;

        for (KVEntry kvEntry : entries) {
            String key = kvEntry.key();
            String revisionStr = key.substring(key.lastIndexOf('/') + 1);
            try {
                long rev = Long.parseLong(revisionStr);
                if (rev <= upToRevision) {
                    kvStore.delete(key);
                    cache.remove(rev);
                    removed++;
                }
            } catch (NumberFormatException e) {
                logger.warn("Skipping non-numeric journal key during compaction: {}", key);
            }
        }

        logger.info("Compacted journal: removed {} entries up to revision {}", removed, upToRevision);
    }

    private String entryKey(long revision) {
        // Zero-pad to 20 digits for lexicographic ordering in KV stores
        return basePath + "/" + String.format("%020d", revision);
    }
}
