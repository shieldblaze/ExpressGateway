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
package com.shieldblaze.expressgateway.configuration.buffer;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.configuration.Configuration;
import dev.morphia.annotations.Entity;
import dev.morphia.annotations.Id;
import dev.morphia.annotations.Property;
import dev.morphia.annotations.Transient;
import io.netty.buffer.PooledByteBufAllocator;
import io.netty.util.internal.PlatformDependent;

import java.util.UUID;

import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkPositive;
import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkZeroOrPositive;

/**
 * Configuration for {@link PooledByteBufAllocator}
 */
@Entity(value = "Buffer", useDiscriminator = false)
public final class BufferConfiguration implements Configuration<BufferConfiguration> {

    @Id
    @JsonProperty
    private String id;

    @Property
    @JsonProperty(required = true)
    private boolean preferDirect;

    @Property
    @JsonProperty(required = true)
    private int heapArena;

    @Property
    @JsonProperty(required = true)
    private int directArena;

    @Property
    @JsonProperty(required = true)
    private int pageSize;

    @Property
    @JsonProperty(required = true)
    private int maxOrder;

    @Property
    @JsonProperty(required = true)
    private int smallCacheSize;

    @Property
    @JsonProperty(required = true)
    private int normalCacheSize;

    @Property
    @JsonProperty(required = true)
    private boolean useCacheForAllThreads;

    @Property
    @JsonProperty(required = true)
    private int directMemoryCacheAlignment;

    @Transient
    @JsonIgnore
    private boolean validated;

    /**
     * Default instance of {@link BufferConfiguration}
     */
    public static final BufferConfiguration DEFAULT = new BufferConfiguration();

    static {
        DEFAULT.id = "default";
        DEFAULT.preferDirect = true;
        DEFAULT.pageSize = 16_384;
        DEFAULT.maxOrder = 11;
        DEFAULT.heapArena = (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                Runtime.getRuntime().maxMemory() / 16384 << 11 / 2 / 3));
        DEFAULT.directArena = (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                PlatformDependent.maxDirectMemory() / 16384 << 11 / 2 / 3));
        DEFAULT.smallCacheSize = 256;
        DEFAULT.normalCacheSize = 64;
        DEFAULT.useCacheForAllThreads = true;
        DEFAULT.directMemoryCacheAlignment = 0;
        DEFAULT.validated = true;
    }

    /**
     * {@code true} to use direct memory else set to {@code false}
     */
    public BufferConfiguration setPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
        return this;
    }

    /**
     * {@code true} to use direct memory else set to {@code false}
     */
    public boolean preferDirect() {
        assertValidated();
        return preferDirect;
    }

    /**
     * Heap Arena Size
     */
    public BufferConfiguration setHeapArena(int heapArena) {
        this.heapArena = heapArena;
        return this;
    }

    /**
     * Heap Arena Size
     */
    public int heapArena() {
        assertValidated();
        return heapArena;
    }

    /**
     * Direct Arena Size
     */
    public BufferConfiguration setDirectArena(int directArena) {
        this.directArena = directArena;
        return this;
    }

    /**
     * Direct Arena Size
     */
    public int directArena() {
        assertValidated();
        return directArena;
    }

    /**
     * Page Size
     */
    public BufferConfiguration setPageSize(int pageSize) {
        this.pageSize = pageSize;
        return this;
    }

    /**
     * Page Size
     */
    public int pageSize() {
        assertValidated();
        return pageSize;
    }

    /**
     * Max Order
     */
    public BufferConfiguration setMaxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
        return this;
    }

    /**
     * Max Order
     */
    public int maxOrder() {
        assertValidated();
        return maxOrder;
    }

    /**
     * Small Cache Size
     */
    public BufferConfiguration setSmallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
        return this;
    }

    /**
     * Small Cache Size
     */
    public int smallCacheSize() {
        assertValidated();
        return smallCacheSize;
    }

    /**
     * Normal Cache Size
     */
    public BufferConfiguration setNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
        return this;
    }

    /**
     * Normal Cache Size
     */
    public int normalCacheSize() {
        assertValidated();
        return normalCacheSize;
    }

    /**
     * {@code true} to use Cache for all threads else set to {@code false}
     */
    public BufferConfiguration setUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
        return this;
    }

    /**
     * {@code true} to use Cache for all threads else set to {@code false}
     */
    public boolean useCacheForAllThreads() {
        assertValidated();
        return useCacheForAllThreads;
    }

    /**
     * Direct Memory Cache Alignment
     */
    public BufferConfiguration setDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
        return this;
    }

    /**
     * Direct Memory Cache Alignment
     */
    public int directMemoryCacheAlignment() {
        assertValidated();
        return directMemoryCacheAlignment;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    @Override
    public BufferConfiguration validate() throws IllegalArgumentException {
        if (id == null) {
            id = UUID.randomUUID().toString();
        } else {
            // Revalidate UUID before using
            id = UUID.fromString(id).toString();
        }
        checkPositive(heapArena, "Heap Arena");
        checkPositive(directArena, "Direct Arena");
        checkPositive(pageSize, "Page Size");
        checkPositive(maxOrder, "Max Order");
        checkPositive(smallCacheSize, "Small Cache Size");
        checkPositive(normalCacheSize, "Normal Cache Size");
        checkZeroOrPositive(directMemoryCacheAlignment, "Direct Memory Cache Alignment");
        validated = true;
        return this;
    }

    @Override
    public String id() {
        return id;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
