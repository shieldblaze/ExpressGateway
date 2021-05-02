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
package com.shieldblaze.expressgateway.configuration.buffer;

import com.shieldblaze.expressgateway.common.utils.Number;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
public final class BufferConfiguration {

    private boolean preferDirect;
    private int heapArena;
    private int directArena;
    private int pageSize;
    private int maxOrder;
    private int smallCacheSize;
    private int normalCacheSize;
    private boolean useCacheForAllThreads;
    private int directMemoryCacheAlignment;

    BufferConfiguration() {
        // Prevent outside initialization
    }

    public static final BufferConfiguration DEFAULT = new BufferConfiguration(
            true,
            16384,
            11,
            (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2, Runtime.getRuntime().maxMemory() / 16384 << 11 / 2 / 3)),
            (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2, PlatformDependent.maxDirectMemory() / 16384 << 11 / 2 / 3)),
            256,
            64,
            true,
            0
    );

    private BufferConfiguration(boolean preferDirect, int pageSize, int maxOrder, int heapArena, int directArena, int smallCacheSize,
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
     * @see BufferConfigurationBuilder#withPreferDirect(boolean)
     */
    public boolean preferDirect() {
        return preferDirect;
    }

    BufferConfiguration preferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withHeapArena(int)
     */
    public int heapArena() {
        return heapArena;
    }

    BufferConfiguration heapArena(int heapArena) {
        Number.checkPositive(heapArena, "heapArena");
        this.heapArena = heapArena;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withDirectArena(int)
     */
    public int directArena() {
        return directArena;
    }

    BufferConfiguration directArena(int directArena) {
        Number.checkPositive(directArena, "directArena");
        this.directArena = directArena;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withPageSize(int)
     */
    public int pageSize() {
        return pageSize;
    }

    BufferConfiguration pageSize(int pageSize) {
        Number.checkPositive(pageSize, "pageSize");
        this.pageSize = pageSize;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withMaxOrder(int)
     */
    public int maxOrder() {
        return maxOrder;
    }

    BufferConfiguration maxOrder(int maxOrder) {
        Number.checkPositive(maxOrder, "maxOrder");
        this.maxOrder = maxOrder;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withSmallCacheSize(int)
     */
    public int smallCacheSize() {
        return smallCacheSize;
    }

    BufferConfiguration smallCacheSize(int smallCacheSize) {
        Number.checkPositive(smallCacheSize, "smallCacheSize");
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withNormalCacheSize(int)
     */
    public int normalCacheSize() {
        return normalCacheSize;
    }

    BufferConfiguration setNormalCacheSize(int normalCacheSize) {
        Number.checkPositive(normalCacheSize, "normalCacheSize");
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withUseCacheForAllThreads(boolean)
     */
    public boolean useCacheForAllThreads() {
        return useCacheForAllThreads;
    }

    BufferConfiguration useCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withDirectMemoryCacheAlignment(int)
     */
    public int directMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    BufferConfiguration directMemoryCacheAlignment(int directMemoryCacheAlignment) {
        Number.checkZeroOrPositive(directMemoryCacheAlignment, "directMemoryCacheAlignment");
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    @Override
    public String toString() {
        return "BufferConfiguration{" +
                "preferDirect=" + preferDirect +
                ", heapArena=" + heapArena +
                ", directArena=" + directArena +
                ", pageSize=" + pageSize +
                ", maxOrder=" + maxOrder +
                ", smallCacheSize=" + smallCacheSize +
                ", normalCacheSize=" + normalCacheSize +
                ", useCacheForAllThreads=" + useCacheForAllThreads +
                ", directMemoryCacheAlignment=" + directMemoryCacheAlignment +
                '}';
    }
}
