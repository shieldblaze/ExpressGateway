/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;

import java.util.Objects;

public final class PooledByteBufAllocator {

    private final io.netty.buffer.PooledByteBufAllocator pooledByteBufAllocator;

    public PooledByteBufAllocator(PooledByteBufAllocatorConfiguration configuration) {
        Objects.requireNonNull(configuration, "PooledByteBufAllocatorConfiguration");

        pooledByteBufAllocator = new io.netty.buffer.PooledByteBufAllocator(
                configuration.rreferDirect(),
                configuration.heapArena(),
                configuration.directArena(),
                configuration.pageSize(),
                configuration.maxOrder(),
                configuration.smallCacheSize(),
                configuration.normalCacheSize(),
                configuration.useCacheForAllThreads(),
                configuration.directMemoryCacheAlignment()
        );
    }

    /**
     * Get Instance of {@link io.netty.buffer.PooledByteBufAllocator}
     */
    public io.netty.buffer.PooledByteBufAllocator instance() {
        return pooledByteBufAllocator;
    }
}
