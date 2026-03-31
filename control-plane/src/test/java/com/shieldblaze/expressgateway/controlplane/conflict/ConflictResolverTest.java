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

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.Map;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConflictResolverTest {

    private ConflictResolver resolver;
    private ConfigResource existing;
    private ConfigResource incoming;

    @BeforeEach
    void setUp() {
        resolver = new ConflictResolver();
        existing = createResource("cluster-1", Instant.parse("2024-01-01T00:00:00Z"), "user-a");
        incoming = createResource("cluster-1", Instant.parse("2024-01-01T00:01:00Z"), "user-b");
    }

    @Test
    void noExistingResourceAlwaysAcceptsIncoming() {
        VectorClock clock = VectorClock.empty().increment("n1");
        Optional<ConfigResource> result = resolver.resolve(null, incoming, null, clock);
        assertTrue(result.isPresent());
        assertSame(incoming, result.get());
    }

    @Test
    void incomingAfterExistingAccepted() {
        VectorClock existingClock = VectorClock.empty().increment("n1");
        VectorClock incomingClock = existingClock.increment("n1");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, existingClock, incomingClock);
        assertTrue(result.isPresent());
        assertSame(incoming, result.get());
    }

    @Test
    void incomingBeforeExistingRejected() {
        VectorClock existingClock = VectorClock.empty().increment("n1").increment("n1");
        VectorClock incomingClock = VectorClock.empty().increment("n1");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, existingClock, incomingClock);
        assertTrue(result.isEmpty());
    }

    @Test
    void equalClocksWithIdenticalContentReturnsExisting() {
        // Create two identical resources
        ConfigResource res1 = createResource("cluster-1", Instant.parse("2024-01-01T00:00:00Z"), "user-a");
        ConfigResource res2 = createResource("cluster-1", Instant.parse("2024-01-01T00:00:00Z"), "user-a");

        VectorClock clock = VectorClock.empty().increment("n1");
        Optional<ConfigResource> result = resolver.resolve(res1, res2, clock, clock);
        assertTrue(result.isPresent());
        assertSame(res1, result.get()); // Returns existing (no-op)
    }

    @Test
    void equalClocksWithDifferentContentTreatedAsConcurrent() {
        // existing and incoming have different timestamps (different content)
        VectorClock clock = VectorClock.empty().increment("n1");
        Optional<ConfigResource> result = resolver.resolve(existing, incoming, clock, clock);
        assertTrue(result.isPresent());
        // With default LWW policy, the resource with later timestamp wins
        assertSame(incoming, result.get()); // incoming has later updatedAt
    }

    @Test
    void equalClocksWithDifferentContentRejectPolicy() {
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Reject());

        VectorClock clock = VectorClock.empty().increment("n1");
        Optional<ConfigResource> result = resolver.resolve(existing, incoming, clock, clock);
        assertTrue(result.isEmpty()); // Reject policy rejects concurrent writes
    }

    @Test
    void concurrentWritesWithLwwPolicyPicksLaterTimestamp() {
        // Default policy is LWW
        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        // incoming has later timestamp
        Optional<ConfigResource> result = resolver.resolve(existing, incoming, a, b);
        assertTrue(result.isPresent());
        assertSame(incoming, result.get());
    }

    @Test
    void concurrentWritesWithLwwPolicyPicksEarlierWhenIncomingOlder() {
        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        // Swap: existing has later timestamp
        ConfigResource olderIncoming = createResource("cluster-1",
                Instant.parse("2023-12-31T00:00:00Z"), "user-b");
        Optional<ConfigResource> result = resolver.resolve(existing, olderIncoming, a, b);
        assertTrue(result.isPresent());
        assertSame(existing, result.get());
    }

    @Test
    void lwwTieBreakerIsDeterministicRegardlessOfOrder() {
        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        // Same timestamp, different createdBy
        Instant sameTime = Instant.parse("2024-01-01T00:00:00Z");
        ConfigResource res1 = createResource("cluster-1", sameTime, "user-a");
        ConfigResource res2 = createResource("cluster-1", sameTime, "user-b");

        // Test both orderings: (res1, res2) and (res2, res1) should produce same winner
        Optional<ConfigResource> result1 = resolver.resolve(res1, res2, a, b);
        Optional<ConfigResource> result2 = resolver.resolve(res2, res1, a, b);

        assertTrue(result1.isPresent());
        assertTrue(result2.isPresent());

        // The same resource should win regardless of which is incoming vs existing.
        // Since both have same name "cluster-1", it falls to createdBy.
        // "user-b" > "user-a" lexicographically, so the resource with "user-b" always wins.
        assertEquals(result1.get().createdBy(), result2.get().createdBy(),
                "LWW tie-breaker must produce the same result regardless of argument order");
    }

    @Test
    void rejectPolicyRejectsConcurrentWrites() {
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Reject());

        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, a, b);
        assertTrue(result.isEmpty());
    }

    @Test
    void mergePolicyMergesConcurrentWrites() {
        // Merge function that takes the incoming resource with a merged label
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Merge(
                (ex, inc) -> new ConfigResource(
                        inc.id(), inc.kind(), inc.scope(), inc.version(),
                        inc.createdAt(), inc.updatedAt(), inc.createdBy(),
                        Map.of("merged", "true"), inc.spec()
                )
        ));

        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, a, b);
        assertTrue(result.isPresent());
        assertEquals("true", result.get().labels().get("merged"));
    }

    @Test
    void mergePolicyRejectsWhenMergeFunctionReturnsNull() {
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Merge((ex, inc) -> null));

        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, a, b);
        assertTrue(result.isEmpty());
    }

    @Test
    void customPolicyDelegatesCorrectly() {
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Custom(
                (ex, inc) -> ex // always keep existing
        ));

        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n2");

        Optional<ConfigResource> result = resolver.resolve(existing, incoming, a, b);
        assertTrue(result.isPresent());
        assertSame(existing, result.get());
    }

    @Test
    void perKindPolicies() {
        resolver.registerPolicy(ConfigKind.CLUSTER, new ConflictPolicy.Reject());
        // LISTENER uses default LWW

        assertEquals(ConflictPolicy.Reject.class, resolver.policyFor(ConfigKind.CLUSTER).getClass());
        assertEquals(ConflictPolicy.LastWriterWins.class, resolver.policyFor(ConfigKind.LISTENER).getClass());
    }

    private static ConfigResource createResource(String name, Instant updatedAt, String createdBy) {
        return new ConfigResource(
                new ConfigResourceId("cluster", "global", name),
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1L,
                Instant.parse("2024-01-01T00:00:00Z"),
                updatedAt,
                createdBy,
                Map.of(),
                new ClusterSpec(name, "round-robin", "default-hc", 10000, 30)
        );
    }
}
