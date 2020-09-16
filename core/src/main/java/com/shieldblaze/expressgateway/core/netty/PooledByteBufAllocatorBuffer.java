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
package com.shieldblaze.expressgateway.core.netty;

import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.ObjectUtil;

public final class PooledByteBufAllocatorBuffer {

    private final PooledByteBufAllocator pooledByteBufAllocator;

    public PooledByteBufAllocatorBuffer(PooledByteBufAllocatorConfiguration configuration) {
        ObjectUtil.checkNotNull(configuration, "PooledByteBufAllocatorConfiguration");

        pooledByteBufAllocator = new PooledByteBufAllocator(
                configuration.isPreferDirect(),
                configuration.getHeapArena(),
                configuration.getDirectArena(),
                configuration.getPageSize(),
                configuration.getMaxOrder(),
                configuration.getSmallCacheSize(),
                configuration.getNormalCacheSize(),
                configuration.isUseCacheForAllThreads(),
                configuration.getDirectMemoryCacheAlignment()
        );
    }

    /**
     * Get Instance of {@link PooledByteBufAllocator}
     */
    public PooledByteBufAllocator getInstance() {
        return pooledByteBufAllocator;
    }
}
