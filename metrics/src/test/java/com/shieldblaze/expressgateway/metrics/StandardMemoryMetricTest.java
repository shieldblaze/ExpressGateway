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
package com.shieldblaze.expressgateway.metrics;

import org.junit.jupiter.api.Test;

import java.util.HashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertNotEquals;

class StandardMemoryMetricTest {

    @Test
    void physicalMemoryUsedTest() {
        MemoryMetric memoryMetric = StandardMemoryMetric.INSTANCE;
        float memoryUsed = memoryMetric.physicalMemoryUsed();

        Map<String, Integer> stringIntegerMap = new HashMap<>();
        for (int i = 0; i < 1_000_000; i++) {
            stringIntegerMap.put(String.valueOf(i), i);
        }

        float memoryUsedB = memoryMetric.physicalMemoryUsed();
        assertNotEquals(memoryUsedB, memoryUsed);
    }
}
