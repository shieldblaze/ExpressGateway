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

package com.shieldblaze.expressgateway.configuration.eventstream;

import com.fasterxml.jackson.annotation.JsonIgnore;
import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.Configuration;

import java.io.Closeable;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/**
 * Configuration for {@link EventStreamConfiguration}.
 * <p>
 * Use {@link EventStreamConfigurationBuilder} for building {@link EventStreamConfiguration} instance.
 */
public final class EventStreamConfiguration implements Configuration, Closeable {

    @JsonProperty("workers")
    private int workers;

    @JsonIgnore
    private ExecutorService executor;

    @JsonIgnore
    private EventStream eventStream;

    public static final EventStreamConfiguration DEFAULT = new EventStreamConfiguration(Runtime.getRuntime().availableProcessors());

    public EventStreamConfiguration() {
        // For deserialization
    }

    EventStreamConfiguration(int workers) {
        this.workers = workers;
        executor = Executors.newFixedThreadPool(workers);
        eventStream = new AsyncEventStream(executor);
    }

    public int workers() {
        return workers;
    }

    public EventStream eventStream() {
        if (eventStream == null) {
            executor = Executors.newFixedThreadPool(workers);
            eventStream = new AsyncEventStream(executor);
        }
        return eventStream;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public EventStreamConfiguration validate() throws IllegalArgumentException {
        NumberUtil.checkPositive(workers, "Workers");
        return this;
    }

    @Override
    public String name() {
        return "EventStreamConfiguration";
    }

    @Override
    public void close() {
        executor.shutdown();
    }
}
