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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

import java.io.IOException;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
public final class BufferConfiguration {

    @JsonProperty("preferDirect")
    private boolean preferDirect;

    @JsonProperty("heapArena")
    private int heapArena;

    @JsonProperty("directArena")
    private int directArena;

    @JsonProperty("pageSize")
    private int pageSize;

    @JsonProperty("maxOrder")
    private int maxOrder;

    @JsonProperty("smallCacheSize")
    private int smallCacheSize;

    @JsonProperty("normalCacheSize")
    private int normalCacheSize;

    @JsonProperty("useCacheForAllThreads")
    private boolean useCacheForAllThreads;

    @JsonProperty("directMemoryCacheAlignment")
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

    BufferConfiguration setPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withHeapArena(int)
     */
    public int heapArena() {
        return heapArena;
    }

    BufferConfiguration setHeapArena(int heapArena) {
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

    BufferConfiguration setDirectArena(int directArena) {
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

    BufferConfiguration setPageSize(int pageSize) {
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

    BufferConfiguration setMaxOrder(int maxOrder) {
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

    BufferConfiguration setSmallCacheSize(int smallCacheSize) {
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

    BufferConfiguration setUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    /**
     * @see BufferConfigurationBuilder#withDirectMemoryCacheAlignment(int)
     */
    public int directMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    BufferConfiguration setDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        Number.checkZeroOrPositive(directMemoryCacheAlignment, "directMemoryCacheAlignment");
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    /**
     * Save this configuration to the file
     *
     * @throws IOException If an error occurs during saving
     */
    public void save() throws IOException {
        ConfigurationMarshaller.save("BufferConfiguration.json", this);
    }

    /**
     * Load this configuration from the file
     *
     * @return {@link BufferConfiguration} Instance
     * @throws IOException If an error occurs during loading
     */
    public static BufferConfiguration load() throws IOException {
        return ConfigurationMarshaller.load("BufferConfiguration.json", BufferConfiguration.class);
    }
}
