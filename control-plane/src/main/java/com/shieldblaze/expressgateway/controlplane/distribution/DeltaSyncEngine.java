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
package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import lombok.extern.log4j.Log4j2;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Computes per-node config deltas from the {@link ChangeJournal}.
 *
 * <p>Given a node's last acknowledged revision, this engine replays journal entries
 * since that revision and merges them into a single {@link ConfigDelta}. Merging
 * deduplicates mutations by resource path: only the last mutation per resource ID
 * survives, which is safe because each mutation is a full-state upsert or a delete.</p>
 *
 * <p>If the node is too far behind (the journal has been compacted past its revision),
 * {@link #computeDelta(long)} returns {@code null} to signal that a full snapshot
 * resync is required instead of an incremental delta.</p>
 */
@Log4j2
public final class DeltaSyncEngine {

    private final ChangeJournal journal;
    private final long maxJournalLag;

    /**
     * @param journal       the change journal to read entries from; must not be null
     * @param maxJournalLag maximum number of journal entries a node can lag behind
     *                      before falling back to a full snapshot. This is a safety
     *                      bound -- the actual fallback also triggers when the journal
     *                      has been compacted past the node's revision.
     */
    public DeltaSyncEngine(ChangeJournal journal, long maxJournalLag) {
        this.journal = Objects.requireNonNull(journal, "journal");
        if (maxJournalLag <= 0) {
            throw new IllegalArgumentException("maxJournalLag must be > 0, got: " + maxJournalLag);
        }
        this.maxJournalLag = maxJournalLag;
    }

    /**
     * Compute the config delta for a node at the given revision.
     *
     * @param nodeLastRevision the last revision the node ACKed (0 means the node
     *                         has never received config)
     * @return a {@link ConfigDelta} with merged mutations since that revision,
     *         an empty delta if the node is already current, or {@code null} if
     *         the node is too far behind and requires a full snapshot
     */
    public ConfigDelta computeDelta(long nodeLastRevision) {
        long currentRevision = journal.currentRevision();

        // Node is already at or ahead of current revision -- nothing to push
        if (nodeLastRevision >= currentRevision) {
            return new ConfigDelta(nodeLastRevision, currentRevision, List.of());
        }

        // Safety check: if the node is lagging beyond maxJournalLag, force snapshot
        if (currentRevision - nodeLastRevision > maxJournalLag) {
            log.info("Node at revision {} exceeds max journal lag {} (current={}), needs full snapshot",
                    nodeLastRevision, maxJournalLag, currentRevision);
            return null;
        }

        try {
            List<ChangeJournal.JournalEntry> entries = journal.entriesSince(nodeLastRevision);

            if (entries.isEmpty()) {
                // Journal has been compacted past the node's revision
                log.info("Node at revision {} too far behind current {}, journal compacted -- needs full snapshot",
                        nodeLastRevision, currentRevision);
                return null;
            }

            // Merge all mutations; last mutation per resource path wins.
            // LinkedHashMap preserves insertion order so the resulting delta
            // reflects the chronological order of final states.
            Map<String, ConfigMutation> merged = new LinkedHashMap<>();
            for (ChangeJournal.JournalEntry entry : entries) {
                for (ConfigMutation mutation : entry.mutations()) {
                    String key = switch (mutation) {
                        case ConfigMutation.Upsert u -> u.resource().id().toPath();
                        case ConfigMutation.Delete d -> d.resourceId().toPath();
                    };
                    merged.put(key, mutation);
                }
            }

            return new ConfigDelta(nodeLastRevision, currentRevision, List.copyOf(merged.values()));
        } catch (Exception e) {
            log.error("Failed to compute delta from revision {}", nodeLastRevision, e);
            return null; // fallback to snapshot on any read error
        }
    }
}
