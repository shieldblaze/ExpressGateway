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
package com.shieldblaze.expressgateway.configuration.versioning;

import java.util.Objects;

/**
 * Semantic version for configuration schemas.
 *
 * <p>Follows semantic versioning (major.minor.patch):</p>
 * <ul>
 *   <li><b>Major</b>: breaking changes (removed fields, changed semantics)</li>
 *   <li><b>Minor</b>: backward-compatible additions (new optional fields)</li>
 *   <li><b>Patch</b>: backward-compatible fixes (validation rule adjustments)</li>
 * </ul>
 *
 * @param major Major version (must be >= 0)
 * @param minor Minor version (must be >= 0)
 * @param patch Patch version (must be >= 0)
 */
public record SchemaVersion(int major, int minor, int patch) implements Comparable<SchemaVersion> {

    public static final SchemaVersion V1_0_0 = new SchemaVersion(1, 0, 0);

    public SchemaVersion {
        if (major < 0) {
            throw new IllegalArgumentException("major must be >= 0, got: " + major);
        }
        if (minor < 0) {
            throw new IllegalArgumentException("minor must be >= 0, got: " + minor);
        }
        if (patch < 0) {
            throw new IllegalArgumentException("patch must be >= 0, got: " + patch);
        }
    }

    /**
     * Check if this version is backward-compatible with the given version.
     * A version is backward-compatible if:
     * <ul>
     *   <li>The major version is the same</li>
     *   <li>This version is greater than or equal to the target</li>
     * </ul>
     *
     * @param other The version to check compatibility against
     * @return {@code true} if this version can read configs written for {@code other}
     */
    public boolean isCompatibleWith(SchemaVersion other) {
        Objects.requireNonNull(other, "other");
        return this.major == other.major && this.compareTo(other) >= 0;
    }

    /**
     * Check if upgrading from the given version to this version is a breaking change.
     *
     * @param from The version to upgrade from
     * @return {@code true} if the major version differs
     */
    public boolean isBreakingUpgradeFrom(SchemaVersion from) {
        Objects.requireNonNull(from, "from");
        return this.major != from.major;
    }

    @Override
    public int compareTo(SchemaVersion other) {
        int cmp = Integer.compare(this.major, other.major);
        if (cmp != 0) return cmp;
        cmp = Integer.compare(this.minor, other.minor);
        if (cmp != 0) return cmp;
        return Integer.compare(this.patch, other.patch);
    }

    /**
     * Parse a version string in the format "major.minor.patch".
     *
     * @param version The version string to parse
     * @return The parsed {@link SchemaVersion}
     * @throws IllegalArgumentException if the format is invalid
     */
    public static SchemaVersion parse(String version) {
        Objects.requireNonNull(version, "version");
        String[] parts = version.split("\\.");
        if (parts.length != 3) {
            throw new IllegalArgumentException("Version must be in format 'major.minor.patch', got: " + version);
        }
        try {
            return new SchemaVersion(
                    Integer.parseInt(parts[0]),
                    Integer.parseInt(parts[1]),
                    Integer.parseInt(parts[2])
            );
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException("Invalid version number in: " + version, e);
        }
    }

    @Override
    public String toString() {
        return major + "." + minor + "." + patch;
    }
}
