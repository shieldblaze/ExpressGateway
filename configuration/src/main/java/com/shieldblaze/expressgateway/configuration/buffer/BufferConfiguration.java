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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.configuration.Configuration;
import io.netty.util.internal.PlatformDependent;
import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkPositive;
import static com.shieldblaze.expressgateway.common.utils.NumberUtil.checkZeroOrPositive;

/**
 * Configuration for Netty buffer allocator settings.
 */
@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class BufferConfiguration implements Configuration<BufferConfiguration> {

    @JsonProperty(required = true)
    private boolean preferDirect;

    @JsonProperty(required = true)
    private int heapArena;

    @JsonProperty(required = true)
    private int directArena;

    @JsonProperty(required = true)
    private int pageSize;

    @JsonProperty(required = true)
    private int maxOrder;

    @JsonProperty(required = true)
    private int smallCacheSize;

    @JsonProperty(required = true)
    private int normalCacheSize;

    @JsonProperty(required = true)
    private boolean useCacheForAllThreads;

    @JsonProperty(required = true)
    private int directMemoryCacheAlignment;

    public static final BufferConfiguration DEFAULT = new BufferConfiguration();

    static {
        DEFAULT.preferDirect = true;
        DEFAULT.pageSize = 16_384;
        DEFAULT.maxOrder = 11;
        DEFAULT.heapArena = (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                Runtime.getRuntime().maxMemory() / (16384L << 11) / 2 / 3));
        DEFAULT.directArena = (int) Math.max(0, Math.min((long) Runtime.getRuntime().availableProcessors() * 2,
                PlatformDependent.maxDirectMemory() / (16384L << 11) / 2 / 3));
        DEFAULT.smallCacheSize = 256;
        DEFAULT.normalCacheSize = 64;
        DEFAULT.useCacheForAllThreads = true;
        DEFAULT.directMemoryCacheAlignment = 0;
    }

    @Override
    public BufferConfiguration validate() {
        checkPositive(heapArena, "Heap Arena");
        checkPositive(directArena, "Direct Arena");
        checkPositive(pageSize, "Page Size");
        checkPositive(maxOrder, "Max Order");
        checkPositive(smallCacheSize, "Small Cache Size");
        checkPositive(normalCacheSize, "Normal Cache Size");
        checkZeroOrPositive(directMemoryCacheAlignment, "Direct Memory Cache Alignment");
        return this;
    }
}
