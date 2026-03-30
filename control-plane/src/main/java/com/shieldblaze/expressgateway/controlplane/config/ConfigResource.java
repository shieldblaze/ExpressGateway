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

import java.time.Instant;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;

/**
 * Universal configuration envelope that wraps any {@link ConfigSpec} payload with
 * metadata for identification, versioning, scoping, and audit.
 *
 * <p>This is the primary unit of configuration storage. Each resource is uniquely
 * identified by its {@link ConfigResourceId}, carries a monotonically increasing
 * version for optimistic concurrency, and includes labels for ad-hoc filtering.</p>
 *
 * @param id        The unique resource identifier (kind + scope + name)
 * @param kind      The config kind with schema version
 * @param scope     The scope at which this resource applies
 * @param version   Monotonically increasing version (must be >= 1)
 * @param createdAt When this resource was first created
 * @param updatedAt When this resource was last modified
 * @param createdBy The principal who created or last modified this resource
 * @param labels    Arbitrary key-value labels for filtering and grouping (defensively copied)
 * @param spec      The typed configuration payload
 */
public record ConfigResource(
        ConfigResourceId id,
        ConfigKind kind,
        ConfigScope scope,
        long version,
        Instant createdAt,
        Instant updatedAt,
        String createdBy,
        Map<String, String> labels,
        ConfigSpec spec
) {

    public ConfigResource {
        Objects.requireNonNull(id, "id");
        Objects.requireNonNull(kind, "kind");
        Objects.requireNonNull(scope, "scope");
        if (version < 1) {
            throw new IllegalArgumentException("version must be >= 1, got: " + version);
        }
        Objects.requireNonNull(createdAt, "createdAt");
        Objects.requireNonNull(updatedAt, "updatedAt");
        Objects.requireNonNull(createdBy, "createdBy");
        Objects.requireNonNull(spec, "spec");
        // Defensive copy of labels to guarantee immutability
        labels = labels == null
                ? Collections.emptyMap()
                : Collections.unmodifiableMap(new LinkedHashMap<>(labels));
    }
}
