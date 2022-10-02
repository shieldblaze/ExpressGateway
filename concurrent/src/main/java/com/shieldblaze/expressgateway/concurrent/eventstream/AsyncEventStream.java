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

import java.util.concurrent.ExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * {@linkplain AsyncEventStream} uses {@link ExecutorService} to
 * publish events asynchronously.
 */
public final class AsyncEventStream extends EventStream {

    /**
     * {@link ExecutorService} for execution of {@link #publish(Event)}
     */
    private final ExecutorService executorService;

    public AsyncEventStream(ExecutorService executorService) {
        this.executorService = executorService;
    }

    /**
     * Publish an Event to all subscribed {@linkplain EventListener} asynchronously.
     *
     * @param event Event to publish
     */
    @SuppressWarnings({"unchecked"})
    @Override
    public void publish(Event event) {
        executorService.execute(() -> subscribers.forEach(eventListener -> executorService.execute(() -> eventListener.accept(event))));
    }

    @Override
    public void close() {
        try {
            executorService.awaitTermination(1, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            throw new IllegalStateException("EventStream executor interrupted while waiting for termination", e);
        } finally {
            executorService.shutdown();
            super.close();
        }
    }
}
