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

import io.netty.util.Recycler;
import stormpot.Pool;
import stormpot.Timeout;

import java.time.Duration;
import java.util.concurrent.ConcurrentHashMap;

public class ConcurrentHashMapPool {

    private ConcurrentHashMapPool() {
        // Prevent outside initialization
    }

    public static final Recycler<ConcurrentHashMap> INSTANCE = new Recycler<>() {
        @Override
        protected ConcurrentHashMap newObject(Handle<ConcurrentHashMap> handle) {
            return new ConcurrentHashMap<>();
        }
    };

    public static final Recycler.Handle<ConcurrentHashMap> HANDLE = ConcurrentHashMap::clear;
}
