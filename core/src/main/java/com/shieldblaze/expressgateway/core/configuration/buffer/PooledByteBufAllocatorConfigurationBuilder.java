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

public final class PooledByteBufAllocatorConfigurationBuilder {
    private boolean preferDirect;
    private int HeapArena;
    private int DirectArena;
    private int pageSize;
    private int maxOrder;
    private int smallCacheSize;
    private int normalCacheSize;
    private boolean useCacheForAllThreads;
    private int directMemoryCacheAlignment;

    private PooledByteBufAllocatorConfigurationBuilder() {
    }

    public static PooledByteBufAllocatorConfigurationBuilder newBuilder() {
        return new PooledByteBufAllocatorConfigurationBuilder();
    }

    public PooledByteBufAllocatorConfigurationBuilder withPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withHeapArena(int HeapArena) {
        this.HeapArena = HeapArena;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withDirectArena(int DirectArena) {
        this.DirectArena = DirectArena;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withPageSize(int pageSize) {
        this.pageSize = pageSize;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withMaxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withSmallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    public PooledByteBufAllocatorConfigurationBuilder withDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    public PooledByteBufAllocatorConfiguration build() {
        PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration = new PooledByteBufAllocatorConfiguration();
        pooledByteBufAllocatorConfiguration.setPreferDirect(preferDirect);
        pooledByteBufAllocatorConfiguration.setHeapArena(HeapArena);
        pooledByteBufAllocatorConfiguration.setDirectArena(DirectArena);
        pooledByteBufAllocatorConfiguration.setPageSize(pageSize);
        pooledByteBufAllocatorConfiguration.setMaxOrder(maxOrder);
        pooledByteBufAllocatorConfiguration.setSmallCacheSize(smallCacheSize);
        pooledByteBufAllocatorConfiguration.setNormalCacheSize(normalCacheSize);
        pooledByteBufAllocatorConfiguration.setUseCacheForAllThreads(useCacheForAllThreads);
        pooledByteBufAllocatorConfiguration.setDirectMemoryCacheAlignment(directMemoryCacheAlignment);
        return pooledByteBufAllocatorConfiguration;
    }
}
