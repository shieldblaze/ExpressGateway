/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.cache;

import io.netty.buffer.ByteBuf;

import java.io.RandomAccessFile;
import java.time.Instant;
import java.util.concurrent.atomic.AtomicLong;

public class Cached {

    /**
     * Cached Data in {@link ByteBuf}
     */
    private ByteBuf byteBuf = null;

    /**
     * Cached Data in {@link RandomAccessFile}
     */
    private RandomAccessFile randomAccessFile = null;

    /**
     * TTL in seconds of the cached data
     */
    private final int CacheTTL;

    private final AtomicLong lastAccessed = new AtomicLong(System.currentTimeMillis());

    public Cached(ByteBuf byteBuf, int TTL) {
        this.byteBuf = byteBuf;
        CacheTTL = TTL;
    }

    public Cached(RandomAccessFile randomAccessFile, int TTL) {
        this.randomAccessFile = randomAccessFile;
        CacheTTL = TTL;
    }

    /**
     * Returns {@code true} if cache is stored in {@link #byteBuf} else {@code false}.
     */
    public boolean isByteBuf() {
        return byteBuf != null;
    }

    /**
     * Returns {@link ByteBuf} of cached data only when {@link #isByteBuf()} is set to {@code true}
     */
    public ByteBuf byteBuf() {
        lastAccessed.set(System.currentTimeMillis()); // Set last accessed timestamp
        return byteBuf;
    }

    /**
     * Returns {@link RandomAccessFile} of cached data only when {@link #isByteBuf()} is set to {@code false}
     */
    public RandomAccessFile randomAccessFile() {
        lastAccessed.set(System.currentTimeMillis()); // Set last accessed timestamp
        return randomAccessFile;
    }

    /**
     * Returns {@code true} if cache has expired
     */
    public boolean hasExpired() {
        return System.currentTimeMillis() > Instant.ofEpochMilli(lastAccessed.get()).plusSeconds(CacheTTL).toEpochMilli();
    }
}
