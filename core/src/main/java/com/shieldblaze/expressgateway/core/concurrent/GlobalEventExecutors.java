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
package com.shieldblaze.expressgateway.core.concurrent;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.function.Supplier;

public final class GlobalEventExecutors {
    private static final ExecutorService EXECUTOR_SERVICE = Executors.newCachedThreadPool();
    public static final GlobalEventExecutors INSTANCE = new GlobalEventExecutors();

    private GlobalEventExecutors() {
        // Prevent outside initialization
    }

    public CompletableFuture<Void> submitTask(Runnable runnable) {
        return CompletableFuture.runAsync(runnable, EXECUTOR_SERVICE);
    }

    public <T> CompletableFuture<T> submitTask(Supplier<T> supplier) {
        return CompletableFuture.supplyAsync(supplier, EXECUTOR_SERVICE);
    }
}
