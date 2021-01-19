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
package com.shieldblaze.expressgateway.concurrent;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.function.Supplier;

/**
 * Global Executors provides various methods to execute a task.
 */
public final class GlobalExecutors {

    /**
     * {@link GlobalExecutors} Instance
     */
    public static final GlobalExecutors INSTANCE = new GlobalExecutors();

    /**
     * Cached {@link ExecutorService}
     */
    private static final ExecutorService EXECUTOR_SERVICE = Executors.newFixedThreadPool(Runtime.getRuntime().availableProcessors());

    /**
     * Scheduled {@link ExecutorService}
     */
    private static final ScheduledExecutorService SCHEDULED_EXECUTOR_SERVICE = Executors.newScheduledThreadPool(Runtime.getRuntime().availableProcessors());

    private GlobalExecutors() {
        // Prevent outside initialization

        // Register Shutdown Hook to shutdown all Executors
        Runtime.getRuntime().addShutdownHook(new Thread(this::shutdownAll));
    }

    /**
     * Submit a new {@link Runnable} task to executed
     *
     * @param runnable {@link Runnable} to be executed
     * @return {@link CompletableFuture} Instance of task to be executed
     */
    public CompletableFuture<Void> submitTask(Runnable runnable) {
        return CompletableFuture.runAsync(runnable, EXECUTOR_SERVICE);
    }

    /**
     * Submit a new task to be executed
     *
     * @param supplier {@link Supplier} implementing task to be executed
     * @param <T>      Class implementing {@link Supplier}
     * @return {@link CompletableFuture} Instance of task to be executed
     */
    public <T> CompletableFuture<T> submitTask(Supplier<T> supplier) {
        return CompletableFuture.supplyAsync(supplier, EXECUTOR_SERVICE);
    }

    /**
     * Submit and schedule a new {@link Runnable} task to be executed with a fixed delay
     *
     * @param runnable {@link Runnable} to be executed
     * @return {@link CompletableFuture} Instance of task to be executed
     */
    public ScheduledFuture<?> submitTaskAndRunEvery(Runnable runnable, int initialDelay, int period, TimeUnit timeUnit) {
        return SCHEDULED_EXECUTOR_SERVICE.scheduleWithFixedDelay(runnable, initialDelay, period, timeUnit);
    }

    public ExecutorService getExecutorService() {
        return EXECUTOR_SERVICE;
    }

    /**
     * Shutdown all {@link Executors}
     */
    public void shutdownAll() {
        EXECUTOR_SERVICE.shutdownNow();
        SCHEDULED_EXECUTOR_SERVICE.shutdownNow();
    }
}
