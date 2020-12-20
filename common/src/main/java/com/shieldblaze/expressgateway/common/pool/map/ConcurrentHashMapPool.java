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
package com.shieldblaze.expressgateway.common.pool.map;

import stormpot.Pool;
import stormpot.Timeout;

import java.time.Duration;

public class ConcurrentHashMapPool {

    private ConcurrentHashMapPool() {
        // Prevent outside initialization
    }

    private static final Pool<ConcurrentHashMapPooled> POOL = Pool.from(new ConcurrentHashMapAllocator())
            .setSize(Integer.MAX_VALUE)
            .setExpiration(info -> {
                return info.getAgeMillis() >= 60000; // 60 Seconds (1 Minute)
            })
            .build();

    public static ConcurrentHashMapPooled newInstance() throws InterruptedException {
        return POOL.claim(new Timeout(Duration.ofSeconds(5)));
    }
}
