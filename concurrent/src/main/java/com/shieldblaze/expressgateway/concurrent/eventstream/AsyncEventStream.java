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
package com.shieldblaze.expressgateway.concurrent.eventstream;

import com.shieldblaze.expressgateway.concurrent.event.Event;

import java.util.concurrent.ExecutorService;

/**
 * {@linkplain AsyncEventStream} uses {@link ExecutorService} to
 * publish events asynchronously.
 */
public class AsyncEventStream extends EventStream {

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
    @Override
    public void publish(Event event) {
        subscribers.forEach(eventListener -> executorService.execute(() -> eventListener.accept(event)));
    }
}
