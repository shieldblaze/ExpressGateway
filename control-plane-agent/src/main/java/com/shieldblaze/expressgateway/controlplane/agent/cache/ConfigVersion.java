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
package com.shieldblaze.expressgateway.controlplane.agent.cache;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.time.Instant;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Objects;

/**
 * Represents a versioned configuration snapshot with integrity metadata.
 *
 * <p>Each version tracks the config revision from the control plane, a SHA-256
 * checksum for integrity verification, and optional metadata (e.g., tags for
 * rollback points).</p>
 *
 * <p>Ordering: versions are ordered by {@code revision} (ascending).</p>
 *
 * @param revision  the global config revision from the control plane
 * @param timestamp when this version was persisted locally
 * @param checksum  SHA-256 hex digest of the serialized config data
 * @param metadata  arbitrary key-value metadata (e.g., "tag" -> "v1.2.0")
 */
public record ConfigVersion(
        @JsonProperty("revision") long revision,
        @JsonProperty("timestamp") Instant timestamp,
        @JsonProperty("checksum") String checksum,
        @JsonProperty("metadata") Map<String, String> metadata
) implements Comparable<ConfigVersion> {

    public ConfigVersion {
        if (revision < 0) {
            throw new IllegalArgumentException("revision must be >= 0, got: " + revision);
        }
        Objects.requireNonNull(timestamp, "timestamp");
        Objects.requireNonNull(checksum, "checksum");
        if (checksum.isBlank()) {
            throw new IllegalArgumentException("checksum must not be blank");
        }
        metadata = metadata == null
                ? Collections.emptyMap()
                : Collections.unmodifiableMap(new LinkedHashMap<>(metadata));
    }

    /**
     * Creates a new ConfigVersion with a tag added to the metadata.
     *
     * @param tag the tag name to add
     * @return a new ConfigVersion with the tag
     */
    public ConfigVersion withTag(String tag) {
        Objects.requireNonNull(tag, "tag");
        Map<String, String> newMeta = new LinkedHashMap<>(metadata);
        newMeta.put("tag", tag);
        return new ConfigVersion(revision, timestamp, checksum, newMeta);
    }

    /**
     * Returns the tag from metadata, if present.
     */
    public String tag() {
        return metadata.getOrDefault("tag", null);
    }

    @Override
    public int compareTo(ConfigVersion other) {
        return Long.compare(this.revision, other.revision);
    }
}
