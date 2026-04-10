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
package com.shieldblaze.expressgateway.coordination;

import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.Objects;

/**
 * Immutable entry stored in the coordination backend.
 *
 * <p>Version semantics: every mutation increments the {@code version} counter for the key.
 * Version 0 is reserved for "does not exist" semantics in CAS operations.
 * The {@code createVersion} is the global transaction ID (e.g. ZK czxid, etcd create_revision)
 * at which the key was first created.</p>
 *
 * @param key           hierarchical path key (e.g. {@code /expressgateway/config/clusters/c1})
 * @param value         opaque value bytes; never null (empty keys use zero-length array)
 * @param version       current modification version (always >= 1 for existing keys)
 * @param createVersion global transaction ID at creation time
 */
public record CoordinationEntry(String key, byte[] value, long version, long createVersion) {

    /**
     * Canonical constructor with null-safety enforcement.
     */
    public CoordinationEntry {
        Objects.requireNonNull(key, "key");
        Objects.requireNonNull(value, "value");
    }

    /**
     * Returns the value decoded as a UTF-8 string.
     *
     * @return UTF-8 decoded value
     */
    public String valueAsString() {
        return new String(value, StandardCharsets.UTF_8);
    }

    /**
     * Defensive copy of value bytes. Use {@link #value()} for zero-copy access
     * when the caller guarantees not to mutate the array.
     *
     * @return a copy of the value byte array
     */
    public byte[] valueCopy() {
        return Arrays.copyOf(value, value.length);
    }

    @Override
    public boolean equals(Object o) {
        if (this == o) return true;
        if (!(o instanceof CoordinationEntry that)) return false;
        return version == that.version
                && createVersion == that.createVersion
                && key.equals(that.key)
                && Arrays.equals(value, that.value);
    }

    @Override
    public int hashCode() {
        int result = Objects.hash(key, version, createVersion);
        result = 31 * result + Arrays.hashCode(value);
        return result;
    }

    @Override
    public String toString() {
        return "CoordinationEntry{key='" + key + "', version=" + version
                + ", createVersion=" + createVersion + ", valueLength=" + value.length + "}";
    }
}
