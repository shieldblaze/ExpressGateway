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
package com.shieldblaze.expressgateway.concurrent.eventstream;

import com.shieldblaze.expressgateway.concurrent.task.Task;
import lombok.extern.log4j.Log4j2;
import org.jctools.queues.MpscUnboundedArrayQueue;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

@Log4j2
public final class AsyncEventStream extends EventStream {

    private final ExecutorService executorService;
    private final MpscUnboundedArrayQueue<Task> pendingTasks;
    private final AtomicInteger drainScheduled = new AtomicInteger(0);

    public AsyncEventStream(ExecutorService executorService) {
        this.executorService = executorService;
        this.pendingTasks = new MpscUnboundedArrayQueue<>(1024);
        log.info("Initialized new AsyncEventStream with Executor: {}", executorService);
    }

    @SuppressWarnings("unchecked")
    @Override
    public void publish(Task task) {
        // offer() on MpscUnboundedArrayQueue always returns true (unbounded), safe to ignore
        boolean offered = pendingTasks.offer(task);
        assert offered : "MpscUnboundedArrayQueue.offer() should never fail";
        if (drainScheduled.getAndIncrement() == 0) {
            executorService.execute(this::drainQueue);
        }
    }

    @SuppressWarnings("unchecked")
    private void drainQueue() {
        do {
            drainScheduled.set(1);
            Task task;
            while ((task = pendingTasks.poll()) != null) {
                Task t = task;
                for (EventListener eventListener : subscribers) {
                    try {
                        eventListener.accept(t);
                    } catch (Exception ex) {
                        log.error("Subscriber {} threw exception processing task", eventListener, ex);
                    }
                }
            }
        } while (!drainScheduled.compareAndSet(1, 0));
    }

    @Override
    public void close() {
        try {
            log.info("Shutting down Executor");
            // Schedule a final drain on the executor to respect the MPSC contract
            // (only the single consumer thread should poll the queue).
            executorService.execute(this::drainQueue);
            executorService.shutdown();

            boolean successfulTermination = executorService.awaitTermination(2, TimeUnit.SECONDS);
            log.info("Executor shutdown result: {}", successfulTermination);

            if (!successfulTermination) {
                log.warn("Executor did not terminate gracefully, forcing shutdown");
                executorService.shutdownNow();
            }
        } catch (InterruptedException e) {
            executorService.shutdownNow();
            Thread.currentThread().interrupt();
            throw new IllegalStateException("EventStream executor interrupted while waiting for termination", e);
        } finally {
            super.close();
        }
    }
}
