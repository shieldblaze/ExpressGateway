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
package com.shieldblaze.expressgateway.controlplane.conflict;

import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;

import java.util.Objects;
import java.util.function.BiFunction;

/**
 * Sealed hierarchy of conflict resolution strategies for concurrent config changes.
 *
 * <p>Each variant defines how to handle the case where two concurrent writes
 * (detected via {@link VectorClock.Comparison#CONCURRENT}) target the same
 * config resource. The default policy is {@link LastWriterWins}.</p>
 */
public sealed interface ConflictPolicy
        permits ConflictPolicy.LastWriterWins,
                ConflictPolicy.Merge,
                ConflictPolicy.Reject,
                ConflictPolicy.Custom {

    /**
     * Last-Writer-Wins: the resource with the later {@code updatedAt} timestamp wins.
     * If timestamps are equal, the resource from the lexicographically greater
     * instance ID (from the vector clock) wins, providing a deterministic tie-breaker.
     */
    record LastWriterWins() implements ConflictPolicy {
    }

    /**
     * Merge: attempts to merge two concurrent changes by delegating to a
     * user-provided merge function. The merge function receives both resources
     * and must return a merged result. If the merge function returns {@code null},
     * the conflict is treated as unresolvable and rejected.
     *
     * @param mergeFunction a function that takes (existing, incoming) and returns merged,
     *                      or {@code null} if merge is not possible
     */
    record Merge(BiFunction<ConfigResource, ConfigResource, ConfigResource> mergeFunction)
            implements ConflictPolicy {
        public Merge {
            Objects.requireNonNull(mergeFunction, "mergeFunction");
        }
    }

    /**
     * Reject: concurrent writes are always rejected. The caller must retry after
     * reading the current state. This provides the strongest consistency guarantee
     * at the cost of availability.
     */
    record Reject() implements ConflictPolicy {
    }

    /**
     * Custom: delegates conflict resolution to an arbitrary resolver function.
     * The function receives (existing, incoming) and returns the winning resource,
     * or {@code null} to reject the write.
     *
     * @param resolver a function that takes (existing, incoming) and returns the winner
     */
    record Custom(BiFunction<ConfigResource, ConfigResource, ConfigResource> resolver)
            implements ConflictPolicy {
        public Custom {
            Objects.requireNonNull(resolver, "resolver");
        }
    }
}
