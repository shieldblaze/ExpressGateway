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
package com.shieldblaze.expressgateway.common.concurrent;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.SynchronousQueue;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.function.Supplier;

public final class GlobalEventExecutors {

    /**
     * Cached {@link ExecutorService}
     */
    private static final ExecutorService EXECUTOR_SERVICE = new ThreadPoolExecutor(0, Integer.MAX_VALUE, 300L, TimeUnit.SECONDS, new SynchronousQueue<>());

    /**
     * Scheduled {@link ExecutorService}
     */
    private static final ScheduledExecutorService SCHEDULED_EXECUTOR_SERVICE = Executors.newScheduledThreadPool(0);

    /**
     * {@link GlobalEventExecutors} Instance
     */
    public static final GlobalEventExecutors INSTANCE = new GlobalEventExecutors();

    private GlobalEventExecutors() {
        // Prevent outside initialization

        // Register Shutdown Hook to shutdown all Executors
        Runtime.getRuntime().addShutdownHook(new Thread(this::shutdownAll));
    }

    public CompletableFuture<Void> submitTask(Runnable runnable) {
        return CompletableFuture.runAsync(runnable, EXECUTOR_SERVICE);
    }

    public <T> CompletableFuture<T> submitTask(Supplier<T> supplier) {
        return CompletableFuture.supplyAsync(supplier, EXECUTOR_SERVICE);
    }

    public ScheduledFuture<?> submitTaskAndRunEvery(Runnable runnable) {
        return SCHEDULED_EXECUTOR_SERVICE.scheduleAtFixedRate(runnable, 10, 10, TimeUnit.SECONDS);
    }

    public ScheduledFuture<?> submitTaskAndRunEvery(Runnable runnable, int initialDelay, int period, TimeUnit timeUnit) {
        return SCHEDULED_EXECUTOR_SERVICE.scheduleWithFixedDelay(runnable, initialDelay, period, timeUnit);
    }

    /**
     * Shutdown all {@link Executors}
     */
    public void shutdownAll() {
        EXECUTOR_SERVICE.shutdownNow();
        SCHEDULED_EXECUTOR_SERVICE.shutdownNow();
    }
}
