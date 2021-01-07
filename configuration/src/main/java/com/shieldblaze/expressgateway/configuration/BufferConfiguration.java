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
package com.shieldblaze.expressgateway.configuration;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
public final class BufferConfiguration implements Configuration {

    public static final BufferConfiguration EMPTY_INSTANCE = new BufferConfiguration();

    @Expose
    @JsonProperty("preferDirect")
    private boolean preferDirect;

    @Expose
    @JsonProperty("heapArena")
    private int heapArena;

    @Expose
    @JsonProperty("directArena")
    private int directArena;

    @Expose
    @JsonProperty("pageSize")
    private int pageSize;

    @Expose
    @JsonProperty("maxOrder")
    private int maxOrder;

    @Expose
    @JsonProperty("smallCacheSize")
    private int smallCacheSize;

    @Expose
    @JsonProperty("normalCacheSize")
    private int normalCacheSize;

    @Expose
    @JsonProperty("useCacheForAllThreads")
    private boolean useCacheForAllThreads;

    @Expose
    @JsonProperty("directMemoryCacheAlignment")
    private int directMemoryCacheAlignment;

    private BufferConfiguration() {
        // Empty constructor
    }

    public static final BufferConfiguration DEFAULT = new BufferConfiguration(
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

    public boolean preferDirect() {
        return preferDirect;
    }

    public BufferConfiguration preferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    public int heapArena() {
        return heapArena;
    }

    public BufferConfiguration heapArena(int heapArena) {
        this.heapArena = heapArena;
        return this;
    }

    public int directArena() {
        return directArena;
    }

    public BufferConfiguration directArena(int directArena) {
        this.directArena = directArena;
        return this;
    }

    public int pageSize() {
        return pageSize;
    }

    public BufferConfiguration pageSize(int pageSize) {
        this.pageSize = pageSize;
        return this;
    }

    public int maxOrder() {
        return maxOrder;
    }

    public BufferConfiguration maxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
        return this;
    }

    public int smallCacheSize() {
        return smallCacheSize;
    }

    public BufferConfiguration smallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    public int normalCacheSize() {
        return normalCacheSize;
    }

    public BufferConfiguration setNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    public boolean useCacheForAllThreads() {
        return useCacheForAllThreads;
    }

    public BufferConfiguration useCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    public int directMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    public BufferConfiguration directMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    public void validate() throws IllegalArgumentException {
        Number.checkPositive(heapArena, "heapArena");
        Number.checkPositive(directArena, "directArena");
        Number.checkPositive(pageSize, "pageSize");
        Number.checkPositive(maxOrder, "maxOrder");
        Number.checkPositive(smallCacheSize, "smallCacheSize");
        Number.checkPositive(normalCacheSize, "normalCacheSize");
        Number.checkZeroOrPositive(directMemoryCacheAlignment, "directMemoryCacheAlignment");
    }

    @Override
    public String name() {
        return "Buffer";
    }
}
