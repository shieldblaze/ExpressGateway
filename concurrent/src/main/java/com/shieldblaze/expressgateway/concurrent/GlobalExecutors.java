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
package com.shieldblaze.expressgateway.concurrent;

import lombok.AccessLevel;
import lombok.NoArgsConstructor;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ThreadFactory;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Supplier;

@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class GlobalExecutors {

    private static final int PROCESSORS = Runtime.getRuntime().availableProcessors();

    public static final GlobalExecutors INSTANCE = new GlobalExecutors();

    private static final ExecutorService EXECUTOR_SERVICE = Executors.newVirtualThreadPerTaskExecutor();

    private static final ScheduledExecutorService SCHEDULED_EXECUTOR_SERVICE =
            Executors.newScheduledThreadPool(PROCESSORS * 2, new ThreadFactory() {
                private final AtomicInteger counter = new AtomicInteger(0);

                @Override
                public Thread newThread(Runnable r) {
                    Thread t = new Thread(r, "eg-scheduled-" + counter.getAndIncrement());
                    t.setDaemon(true);
                    return t;
                }
            });

    public static CompletableFuture<Void> submitTask(Runnable runnable) {
        return CompletableFuture.runAsync(runnable, EXECUTOR_SERVICE);
    }

    public static <T> CompletableFuture<T> submitTask(Supplier<T> supplier) {
        return CompletableFuture.supplyAsync(supplier, EXECUTOR_SERVICE);
    }

    public static ScheduledFuture<?> submitTaskAndRunEvery(Runnable runnable, int initialDelay, int period, TimeUnit timeUnit) {
        return SCHEDULED_EXECUTOR_SERVICE.scheduleWithFixedDelay(runnable, initialDelay, period, timeUnit);
    }

    public static ExecutorService executorService() {
        return EXECUTOR_SERVICE;
    }

    public static ScheduledExecutorService scheduledExecutorService() {
        return SCHEDULED_EXECUTOR_SERVICE;
    }

    public void shutdownAll() {
        EXECUTOR_SERVICE.shutdown();
        SCHEDULED_EXECUTOR_SERVICE.shutdown();
        try {
            if (!EXECUTOR_SERVICE.awaitTermination(10, TimeUnit.SECONDS)) {
                EXECUTOR_SERVICE.shutdownNow();
            }
            if (!SCHEDULED_EXECUTOR_SERVICE.awaitTermination(10, TimeUnit.SECONDS)) {
                SCHEDULED_EXECUTOR_SERVICE.shutdownNow();
            }
        } catch (InterruptedException e) {
            EXECUTOR_SERVICE.shutdownNow();
            SCHEDULED_EXECUTOR_SERVICE.shutdownNow();
            Thread.currentThread().interrupt();
        }
    }
}
