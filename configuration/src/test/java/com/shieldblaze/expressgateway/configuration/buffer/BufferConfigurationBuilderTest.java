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

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class BufferConfigurationBuilderTest {

    @Test
    void heapArenaTest() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder().withHeapArena(-1).build());
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder().withHeapArena(0).build());
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder().withHeapArena(Integer.MIN_VALUE).build());
    }

    @Test
    void directArenaTest() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(Integer.MIN_VALUE)
                .build());
    }

    @Test
    void pageSizeTest() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(Integer.MIN_VALUE)
                .build());
    }

    @Test
    void maxOrderTest() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(Integer.MIN_VALUE)
                .build());
    }

    @Test
    void smallCacheSize() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(Integer.MIN_VALUE)
                .build());
    }

    @Test
    void normalCacheSize() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(0)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(Integer.MIN_VALUE)
                .build());
    }

    @Test
    void directMemoryCacheAlignmentTest() {
        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(1)
                .withDirectMemoryCacheAlignment(-1)
                .build());

        assertThrows(IllegalArgumentException.class, () -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(1)
                .withDirectMemoryCacheAlignment(Integer.MIN_VALUE)
                .build());

        assertDoesNotThrow(() -> BufferConfigurationBuilder.newBuilder()
                .withHeapArena(1)
                .withDirectArena(1)
                .withPageSize(1)
                .withMaxOrder(1)
                .withSmallCacheSize(1)
                .withNormalCacheSize(1)
                .withDirectMemoryCacheAlignment(0)
                .build());
    }
}
