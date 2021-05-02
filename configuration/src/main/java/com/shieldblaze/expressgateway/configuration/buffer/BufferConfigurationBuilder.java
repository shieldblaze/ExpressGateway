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

/**
 * Configuration Builder for {@link BufferConfiguration}
 */
public final class BufferConfigurationBuilder {
    private boolean preferDirect;
    private int heapArena;
    private int directArena;
    private int pageSize;
    private int maxOrder;
    private int smallCacheSize;
    private int normalCacheSize;
    private boolean useCacheForAllThreads;
    private int directMemoryCacheAlignment;

    private BufferConfigurationBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link BufferConfigurationBuilder} Instance
     */
    public static BufferConfigurationBuilder newBuilder() {
        return new BufferConfigurationBuilder();
    }

    /**
     * {@code true} to use direct memory else set to {@code false}
     */
    public BufferConfigurationBuilder withPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    /**
     * Heap Arena Size
     */
    public BufferConfigurationBuilder withHeapArena(int HeapArena) {
        this.heapArena = HeapArena;
        return this;
    }

    /**
     * Direct Arena Size
     */
    public BufferConfigurationBuilder withDirectArena(int DirectArena) {
        this.directArena = DirectArena;
        return this;
    }

    /**
     * Page Size
     */
    public BufferConfigurationBuilder withPageSize(int pageSize) {
        this.pageSize = pageSize;
        return this;
    }

    /**
     * Max Order
     */
    public BufferConfigurationBuilder withMaxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
        return this;
    }

    /**
     * Small Cache Size
     */
    public BufferConfigurationBuilder withSmallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    /**
     * Normal Cache Size
     */
    public BufferConfigurationBuilder withNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    /**
     * {@code true} to use Cache for all threads else set to {@code false}
     */
    public BufferConfigurationBuilder withUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    /**
     * Direct Memory Cache Alignment
     */
    public BufferConfigurationBuilder withDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    /**
     * Build {@link BufferConfiguration}
     *
     * @return {@link BufferConfiguration} Instance
     * @throws IllegalArgumentException If a required value is invalid
     */
    public BufferConfiguration build() {
        return new BufferConfiguration()
                .preferDirect(preferDirect)
                .heapArena(heapArena)
                .directArena(directArena)
                .pageSize(pageSize)
                .maxOrder(maxOrder)
                .smallCacheSize(smallCacheSize)
                .setNormalCacheSize(normalCacheSize)
                .useCacheForAllThreads(useCacheForAllThreads)
                .directMemoryCacheAlignment(directMemoryCacheAlignment);
    }
}
