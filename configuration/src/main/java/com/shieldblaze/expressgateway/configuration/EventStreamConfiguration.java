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

package com.shieldblaze.expressgateway.configuration;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

import java.util.concurrent.Executors;

public class EventStreamConfiguration implements Configuration {

    public static final EventStreamConfiguration EMPTY_INSTANCE = new EventStreamConfiguration();

    private EventStream eventStream;

    @Expose
    @JsonProperty("workers")
    private int workers;

    private EventStreamConfiguration() {
        // Prevent outside initialization
    }

    public EventStreamConfiguration(int workers) {
        this.workers = workers;
        if (workers == 0) {
            eventStream = new EventStream();
        } else {
            eventStream = new AsyncEventStream(Executors.newFixedThreadPool(workers));
        }
    }

    public EventStream eventStream() {
        return eventStream;
    }

    public int workers() {
        return workers;
    }

    @Override
    public String name() {
        return "EventStream";
    }

    @Override
    public void validate() throws IllegalArgumentException {
        Number.checkZeroOrPositive(workers, "Workers");
    }
}