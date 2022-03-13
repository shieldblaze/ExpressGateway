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
package com.shieldblaze.expressgateway.common.map;

import org.junit.jupiter.api.Test;

import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;

import static org.junit.jupiter.api.Assertions.assertEquals;

class ManualExpiringMapTest {

    @Test
    public void testExpiring() throws Exception {
        ExpiringMap<String, String> expiringMap = new ManualExpiringMap<>(new ConcurrentHashMap<>(), Duration.ofSeconds(5), true);

        // Add entries
        for (int i = 0; i < 100; i++) {
            expiringMap.put("Meow" + i, "Cat" + i);
        }

        // Verify entries
        for (int i = 0; i < 100; i++) {
            assertEquals("Cat" + i, expiringMap.get("Meow" + i));
        }

        // Verify map size
        assertEquals(100, expiringMap.size());

        Thread.sleep(1000 * 10); // Wait for 10 seconds
        expiringMap.toString(); // Query the map to trigger cleanup

        // Verify map size
        assertEquals(0, expiringMap.size());
    }
}
