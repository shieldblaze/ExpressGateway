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

import com.shieldblaze.expressgateway.concurrent.event.Event;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * {@linkplain AsyncEventStream} uses {@link ExecutorService} to
 * publish events asynchronously.
 */
public final class AsyncEventStream extends EventStream {

    private static final Logger logger = LogManager.getLogger(AsyncEventStream.class);

    /**
     * {@link ExecutorService} for execution of {@link #publish(Event)}
     */
    private final ExecutorService executorService;

    public AsyncEventStream(ExecutorService executorService) {
        this.executorService = executorService;
        logger.info("Initialized new AsyncEventStream with Executor: {}", executorService);
    }

    /**
     * Publish an Event to all subscribed {@linkplain EventListener} asynchronously.
     *
     * @param event Event to publish
     */
    @SuppressWarnings({"unchecked"})
    @Override
    public void publish(Event event) {
        executorService.execute(() -> subscribers.forEach(eventListener -> eventListener.accept(event)));
    }

    @Override
    public void close() {
        try {
            logger.info("Trying to shutdown Executor");

            // Await for termination
            boolean successfulTermination = executorService.awaitTermination(2, TimeUnit.SECONDS);
            logger.info("Executor shutdown result: {}", successfulTermination);
        } catch (InterruptedException e) {
            throw new IllegalStateException("EventStream executor interrupted while waiting for termination", e);
        } finally {
            logger.info("Shutting down Executor");

            executorService.shutdown();
            super.close();
        }
    }
}
