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

import java.util.concurrent.Executors;

/**
 * Configuration for {@link EventStreamConfiguration}
 */
public final class EventStreamConfiguration implements Configuration<EventStreamConfiguration> {

    @JsonProperty
    private int workers;

    @JsonIgnore
    private boolean validated;

    public static final EventStreamConfiguration DEFAULT = new EventStreamConfiguration();

    static {
        DEFAULT.workers = Runtime.getRuntime().availableProcessors() * 2;
        DEFAULT.validated = true;
    }

    /**
     * Workers
     */
    public EventStreamConfiguration setWorkers(int workers) {
        this.workers = workers;
        return this;
    }

    /**
     * Workers
     */
    public int workers() {
        assertValidated();
        return workers;
    }

    public EventStream newEventStream() {
        return new AsyncEventStream(Executors.newFixedThreadPool(workers));
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    @Override
    public EventStreamConfiguration validate() {
        NumberUtil.checkPositive(workers, "Workers");
        validated = true;
        return this;
    }

    @Override
    public boolean validated() {
        return validated;
    }
}
