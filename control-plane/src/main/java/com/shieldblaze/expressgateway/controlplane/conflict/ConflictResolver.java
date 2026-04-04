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
import lombok.extern.log4j.Log4j2;

import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Resolves conflicts when concurrent config writes target the same resource.
 *
 * <p>Conflict detection uses {@link VectorClock} comparison. When two writes are
 * determined to be {@link VectorClock.Comparison#CONCURRENT}, the configured
 * {@link ConflictPolicy} for the resource's {@link ConfigKind} determines the outcome:</p>
 * <ul>
 *   <li>{@link ConflictPolicy.LastWriterWins} -- the resource with the later timestamp wins</li>
 *   <li>{@link ConflictPolicy.Merge} -- a merge function attempts to combine both changes</li>
 *   <li>{@link ConflictPolicy.Reject} -- the incoming write is rejected</li>
 *   <li>{@link ConflictPolicy.Custom} -- a custom resolver function determines the outcome</li>
 * </ul>
 *
 * <p>If no policy is registered for a given kind, {@link ConflictPolicy.LastWriterWins}
 * is used as the default, matching the existing behavior where the latest write always wins.</p>
 *
 * <p>Thread safety: this class is thread-safe. Policy registration and resolution
 * can be called from any thread.</p>
 */
@Log4j2
public final class ConflictResolver {

    private static final ConflictPolicy DEFAULT_POLICY = new ConflictPolicy.LastWriterWins();

    private final Map<String, ConflictPolicy> policies = new ConcurrentHashMap<>();

    /**
     * Registers a conflict policy for the given config kind.
     * Overwrites any previously registered policy for that kind.
     *
     * @param kind   the config kind to register the policy for
     * @param policy the conflict policy to apply for this kind
     */
    public void registerPolicy(ConfigKind kind, ConflictPolicy policy) {
        Objects.requireNonNull(kind, "kind");
        Objects.requireNonNull(policy, "policy");
        policies.put(kind.name(), policy);
        log.info("Registered conflict policy {} for kind '{}'", policy.getClass().getSimpleName(), kind.name());
    }

    /**
     * Returns the policy registered for the given kind, or the default LWW policy.
     *
     * @param kind the config kind to look up
     * @return the registered policy or {@link ConflictPolicy.LastWriterWins} as default
     */
    public ConflictPolicy policyFor(ConfigKind kind) {
        Objects.requireNonNull(kind, "kind");
        return policies.getOrDefault(kind.name(), DEFAULT_POLICY);
    }

    /**
     * Resolves a potential conflict between an existing resource and an incoming write.
     *
     * <p>Returns the resource that should be stored. The resolution logic is:</p>
     * <ol>
     *   <li>If there is no existing resource, the incoming resource always wins.</li>
     *   <li>If the incoming clock is {@link VectorClock.Comparison#AFTER} the existing,
     *       the incoming resource wins (no conflict).</li>
     *   <li>If the incoming clock is {@link VectorClock.Comparison#BEFORE} the existing,
     *       the incoming resource is stale and should be discarded.</li>
     *   <li>If the incoming clock is {@link VectorClock.Comparison#EQUAL}:
     *       if the content is identical, return existing (no-op); if content differs,
     *       treat as CONCURRENT and apply the configured conflict policy.</li>
     *   <li>If the clocks are {@link VectorClock.Comparison#CONCURRENT}, the configured
     *       {@link ConflictPolicy} for the resource kind determines the outcome.</li>
     * </ol>
     *
     * @param existing      the current resource in the store (null if none)
     * @param incoming      the incoming write
     * @param existingClock the vector clock of the existing resource
     * @param incomingClock the vector clock of the incoming write
     * @return the resolved resource to store, or empty if the write should be rejected
     */
    public Optional<ConfigResource> resolve(
            ConfigResource existing,
            ConfigResource incoming,
            VectorClock existingClock,
            VectorClock incomingClock) {

        Objects.requireNonNull(incoming, "incoming");
        Objects.requireNonNull(incomingClock, "incomingClock");

        // No existing resource -- incoming always wins
        if (existing == null || existingClock == null) {
            return Optional.of(incoming);
        }

        VectorClock.Comparison comparison = incomingClock.compareTo(existingClock);

        return switch (comparison) {
            case AFTER -> Optional.of(incoming);
            case EQUAL -> {
                // If content is identical, return existing (no-op)
                if (contentEquals(existing, incoming)) {
                    log.debug("Equal clocks with identical content for {}, returning existing (no-op)",
                            incoming.id().toPath());
                    yield Optional.of(existing);
                }
                // Content differs with equal clocks: treat as CONCURRENT
                log.info("Equal clocks with different content for {}, treating as concurrent",
                        incoming.id().toPath());
                yield resolveConcurrent(existing, incoming);
            }
            case BEFORE -> {
                log.debug("Incoming write for {} is causally before existing, discarding",
                        incoming.id().toPath());
                yield Optional.empty();
            }
            case CONCURRENT -> resolveConcurrent(existing, incoming);
        };
    }

    /**
     * Checks if two resources have identical content (payload and metadata).
     * Compares all fields that constitute the "content" of a resource.
     */
    private boolean contentEquals(ConfigResource existing, ConfigResource incoming) {
        return Objects.equals(existing.spec(), incoming.spec())
                && Objects.equals(existing.labels(), incoming.labels())
                && existing.version() == incoming.version()
                && Objects.equals(existing.updatedAt(), incoming.updatedAt())
                && Objects.equals(existing.createdBy(), incoming.createdBy());
    }

    /**
     * Applies the configured conflict policy for two concurrent writes.
     */
    private Optional<ConfigResource> resolveConcurrent(ConfigResource existing, ConfigResource incoming) {
        ConflictPolicy policy = policies.getOrDefault(incoming.kind().name(), DEFAULT_POLICY);
        String resourcePath = incoming.id().toPath();

        log.info("Concurrent write detected for {}, applying policy {}", resourcePath,
                policy.getClass().getSimpleName());

        return switch (policy) {
            case ConflictPolicy.LastWriterWins() -> {
                // Compare updatedAt timestamps; tie-break on resource name lexicographic order
                // for deterministic results regardless of which side is incoming vs existing
                int cmp = incoming.updatedAt().compareTo(existing.updatedAt());
                if (cmp > 0) {
                    yield Optional.of(incoming);
                } else if (cmp < 0) {
                    yield Optional.of(existing);
                } else {
                    // Timestamp tie: use resource name as deterministic tie-breaker.
                    // Compare names (not createdBy) to ensure the same result regardless
                    // of which resource is 'incoming' vs 'existing'.
                    int nameCompare = incoming.id().name().compareTo(existing.id().name());
                    if (nameCompare != 0) {
                        yield nameCompare > 0 ? Optional.of(incoming) : Optional.of(existing);
                    }
                    // Same name: fall back to createdBy for deterministic ordering
                    int createdByCompare = incoming.createdBy().compareTo(existing.createdBy());
                    if (createdByCompare != 0) {
                        yield createdByCompare > 0 ? Optional.of(incoming) : Optional.of(existing);
                    }
                    // Exact tie: pick the one with the higher version
                    yield incoming.version() >= existing.version()
                            ? Optional.of(incoming)
                            : Optional.of(existing);
                }
            }
            case ConflictPolicy.Merge merge -> {
                ConfigResource merged = merge.mergeFunction().apply(existing, incoming);
                if (merged == null) {
                    log.warn("Merge function returned null for {}, rejecting write", resourcePath);
                    yield Optional.empty();
                }
                yield Optional.of(merged);
            }
            case ConflictPolicy.Reject() -> {
                log.warn("Rejecting concurrent write for {} (Reject policy)", resourcePath);
                yield Optional.empty();
            }
            case ConflictPolicy.Custom custom -> {
                ConfigResource result = custom.resolver().apply(existing, incoming);
                if (result == null) {
                    log.warn("Custom resolver returned null for {}, rejecting write", resourcePath);
                    yield Optional.empty();
                }
                yield Optional.of(result);
            }
        };
    }
}
