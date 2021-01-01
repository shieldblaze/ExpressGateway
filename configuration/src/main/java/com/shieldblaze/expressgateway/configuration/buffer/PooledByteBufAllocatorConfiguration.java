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
package com.shieldblaze.expressgateway.configuration.buffer;

import com.google.gson.annotations.Expose;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
public final class PooledByteBufAllocatorConfiguration {

    @Expose
    private boolean preferDirect;

    @Expose
    private int heapArena;

    @Expose
    private int directArena;

    @Expose
    private int pageSize;

    @Expose
    private int maxOrder;

    @Expose
    private int smallCacheSize;

    @Expose
    private int normalCacheSize;

    @Expose
    private boolean useCacheForAllThreads;

    @Expose
    private int directMemoryCacheAlignment;

    PooledByteBufAllocatorConfiguration() {
        // Prevent outside initialization
    }

    public static final PooledByteBufAllocatorConfiguration DEFAULT = new PooledByteBufAllocatorConfiguration(
            true,
            16384,
            11,
            (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                    Runtime.getRuntime().maxMemory() / 16384 << 11 / 2 / 3)),
            (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                    PlatformDependent.maxDirectMemory() / 16384 << 11 / 2 / 3)),
            256,
            64,
            true,
            0
    );

    private PooledByteBufAllocatorConfiguration(boolean preferDirect, int pageSize, int maxOrder, int heapArena, int directArena, int smallCacheSize,
                                                int normalCacheSize, boolean useCacheForAllThreads, int directMemoryCacheAlignment) {
        this.preferDirect = preferDirect;
        this.heapArena = heapArena;
        this.directArena = directArena;
        this.pageSize = pageSize;
        this.maxOrder = maxOrder;
        this.smallCacheSize = smallCacheSize;
        this.normalCacheSize = normalCacheSize;
        this.useCacheForAllThreads = useCacheForAllThreads;
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withPreferDirect(boolean)
     */
    public boolean preferDirect() {
        return preferDirect;
    }

    PooledByteBufAllocatorConfiguration preferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withHeapArena(int)
     */
    public int heapArena() {
        return heapArena;
    }

    PooledByteBufAllocatorConfiguration heapArena(int heapArena) {
        this.heapArena = heapArena;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withDirectArena(int)
     */
    public int directArena() {
        return directArena;
    }

    PooledByteBufAllocatorConfiguration directArena(int directArena) {
        this.directArena = directArena;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withPageSize(int)
     */
    public int pageSize() {
        return pageSize;
    }

    PooledByteBufAllocatorConfiguration pageSize(int pageSize) {
        this.pageSize = pageSize;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withMaxOrder(int)
     */
    public int maxOrder() {
        return maxOrder;
    }

    PooledByteBufAllocatorConfiguration maxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withSmallCacheSize(int)
     */
    public int smallCacheSize() {
        return smallCacheSize;
    }

    PooledByteBufAllocatorConfiguration smallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withNormalCacheSize(int)
     */
    public int normalCacheSize() {
        return normalCacheSize;
    }

    PooledByteBufAllocatorConfiguration setNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withUseCacheForAllThreads(boolean)
     */
    public boolean useCacheForAllThreads() {
        return useCacheForAllThreads;
    }

    PooledByteBufAllocatorConfiguration useCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withDirectMemoryCacheAlignment(int)
     */
    public int directMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    PooledByteBufAllocatorConfiguration directMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }
}
