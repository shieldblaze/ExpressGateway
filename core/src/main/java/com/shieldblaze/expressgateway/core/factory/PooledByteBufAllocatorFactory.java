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
package com.shieldblaze.expressgateway.core.factory;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import io.netty.buffer.PooledByteBufAllocator;
import lombok.Getter;
import lombok.experimental.Accessors;

/**
 * This class provides configured {@link PooledByteBufAllocator} instance.
 *
 * <p>Uses Netty's pooled allocator with arena-based memory management for
 * high-throughput buffer allocation with minimal contention. The configuration
 * controls direct vs heap preference, arena counts, page sizes, and thread-local
 * caching behavior.</p>
 */
@Getter
@Accessors(fluent = true)
public final class PooledByteBufAllocatorFactory {

    private final PooledByteBufAllocator instance;

    /**
     * Create a new {@link PooledByteBufAllocatorFactory} instance
     *
     * @param bufferConfiguration {@link BufferConfiguration} instance
     */
    @NonNull
    public PooledByteBufAllocatorFactory(BufferConfiguration bufferConfiguration) {
        instance = new PooledByteBufAllocator(
                bufferConfiguration.preferDirect(),
                bufferConfiguration.heapArena(),
                bufferConfiguration.directArena(),
                bufferConfiguration.pageSize(),
                bufferConfiguration.maxOrder(),
                bufferConfiguration.smallCacheSize(),
                bufferConfiguration.normalCacheSize(),
                bufferConfiguration.useCacheForAllThreads(),
                bufferConfiguration.directMemoryCacheAlignment()
        );
    }
}
