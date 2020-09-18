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
package com.shieldblaze.expressgateway.core.configuration.buffer;

import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
public final class PooledByteBufAllocatorConfiguration {
    private boolean preferDirect;
    private int HeapArena;
    private int DirectArena;
    private int pageSize;
    private int maxOrder;
    private int smallCacheSize;
    private int normalCacheSize;
    private boolean useCacheForAllThreads;
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
        HeapArena = heapArena;
        DirectArena = directArena;
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
    public boolean isPreferDirect() {
        return preferDirect;
    }

    void setPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withHeapArena(int)
     */
    public int getHeapArena() {
        return HeapArena;
    }

    void setHeapArena(int heapArena) {
        this.HeapArena = heapArena;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withDirectArena(int)
     */
    public int getDirectArena() {
        return DirectArena;
    }

    void setDirectArena(int directArena) {
        this.DirectArena = directArena;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withPageSize(int)
     */
    public int getPageSize() {
        return pageSize;
    }

    void setPageSize(int pageSize) {
        this.pageSize = pageSize;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withMaxOrder(int)
     */
    public int getMaxOrder() {
        return maxOrder;
    }

    void setMaxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withSmallCacheSize(int)
     */
    public int getSmallCacheSize() {
        return smallCacheSize;
    }

    void setSmallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withNormalCacheSize(int)
     */
    public int getNormalCacheSize() {
        return normalCacheSize;
    }

    void setNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withUseCacheForAllThreads(boolean)
     */
    public boolean isUseCacheForAllThreads() {
        return useCacheForAllThreads;
    }

    void setUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
    }

    /**
     * @see PooledByteBufAllocatorConfigurationBuilder#withDirectMemoryCacheAlignment(int)
     */
    public int getDirectMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    void setDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
    }
}
