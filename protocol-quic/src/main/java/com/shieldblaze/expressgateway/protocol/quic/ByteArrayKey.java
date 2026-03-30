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
package com.shieldblaze.expressgateway.protocol.quic;

import java.util.Arrays;
import java.util.HexFormat;

/**
 * Wrapper for {@code byte[]} with proper {@link #equals(Object)} and {@link #hashCode()}
 * for use as a key in {@link java.util.concurrent.ConcurrentHashMap}.
 *
 * <p>Raw byte arrays use identity-based equality by default, making them unsuitable
 * as map keys. This wrapper precomputes the hash code from the array contents
 * (via {@link Arrays#hashCode(byte[])}) for efficient lookups.</p>
 *
 * <p>Used by {@link QuicCidSessionMap} to map QUIC Connection IDs (which are
 * variable-length byte sequences per RFC 9000 Section 5.1) to backend sessions.</p>
 */
public final class ByteArrayKey {

    private final byte[] data;
    private final int hashCode;

    public ByteArrayKey(byte[] data) {
        this.data = data;
        this.hashCode = Arrays.hashCode(data);
    }

    public byte[] data() {
        return data;
    }

    @Override
    public boolean equals(Object o) {
        if (this == o) return true;
        if (!(o instanceof ByteArrayKey that)) return false;
        return Arrays.equals(data, that.data);
    }

    @Override
    public int hashCode() {
        return hashCode;
    }

    @Override
    public String toString() {
        return "ByteArrayKey[" + HexFormat.of().formatHex(data) + "]";
    }
}
