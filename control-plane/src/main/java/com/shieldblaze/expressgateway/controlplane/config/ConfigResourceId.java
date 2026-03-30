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

import java.util.Objects;

/**
 * Unique identifier for a configuration resource, composed of the triple
 * (kind, scopeQualifier, name). This forms a hierarchical path suitable
 * for KV store key construction.
 *
 * <p>Path format: {@code kind/scopeQualifier/name}</p>
 *
 * <p>Components must not contain '/' or '\0' characters to ensure
 * safe path construction and KV store key compatibility.</p>
 *
 * @param kind           The config kind name (e.g. "cluster", "listener")
 * @param scopeQualifier The scope qualifier (e.g. "global", "cluster:prod-1")
 * @param name           The resource name within this kind and scope
 */
public record ConfigResourceId(String kind, String scopeQualifier, String name) {

    public ConfigResourceId {
        Objects.requireNonNull(kind, "kind");
        Objects.requireNonNull(scopeQualifier, "scopeQualifier");
        Objects.requireNonNull(name, "name");
        validateComponent(kind, "kind");
        validateComponent(scopeQualifier, "scopeQualifier");
        validateComponent(name, "name");
    }

    private static void validateComponent(String value, String fieldName) {
        if (value.isBlank()) {
            throw new IllegalArgumentException(fieldName + " must not be blank");
        }
        if (value.indexOf('/') >= 0) {
            throw new IllegalArgumentException(fieldName + " must not contain '/': " + value);
        }
        if (value.indexOf('\0') >= 0) {
            throw new IllegalArgumentException(fieldName + " must not contain null character");
        }
    }

    /**
     * Returns the path representation of this resource ID.
     *
     * @return The path string in the format "kind/scopeQualifier/name"
     */
    public String toPath() {
        return kind + "/" + scopeQualifier + "/" + name;
    }

    /**
     * Parse a path string back into a {@link ConfigResourceId}.
     *
     * @param path The path to parse (format: "kind/scopeQualifier/name")
     * @return The parsed {@link ConfigResourceId}
     * @throws IllegalArgumentException if the path does not contain exactly 3 segments
     */
    public static ConfigResourceId fromPath(String path) {
        Objects.requireNonNull(path, "path");
        String[] parts = path.split("/", 3);
        if (parts.length != 3) {
            throw new IllegalArgumentException(
                    "Path must have exactly 3 segments (kind/scopeQualifier/name), got: " + path);
        }
        return new ConfigResourceId(parts[0], parts[1], parts[2]);
    }
}
