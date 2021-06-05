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

package com.shieldblaze.expressgateway.configuration.eventstream;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;
import java.util.concurrent.Executors;

public class EventStreamConfiguration {

    @JsonProperty("workers")
    private int workers;

    public static final EventStreamConfiguration DEFAULT = new EventStreamConfiguration().setWorkers(Runtime.getRuntime().availableProcessors() / 2);

    public EventStream newEventStream() {
        EventStream eventStream;
        if (workers == 0) {
            eventStream = new EventStream();
        } else {
            eventStream = new AsyncEventStream(Executors.newFixedThreadPool(workers));
        }
        return eventStream;
    }

    EventStreamConfiguration setWorkers(int workers) {
        NumberUtil.checkZeroOrPositive(workers, "Workers");
        this.workers = workers;
        return this;
    }

    public int workers() {
        return workers;
    }

    public EventStreamConfiguration validate() {
        NumberUtil.checkZeroOrPositive(workers, "Workers");
        return this;
    }

    /**
     * Save this configuration to the file
     *
     * @throws IOException If an error occurs during saving
     */
    public void save() throws IOException {
        ConfigurationMarshaller.save("EventStreamConfiguration.json", this);
    }

    /**
     * Load a configuration
     *
     * @return {@link EventStreamConfiguration} Instance
     */
    public static EventStreamConfiguration load() {
        try {
            return ConfigurationMarshaller.load("EventStreamConfiguration.json", EventStreamConfiguration.class);
        } catch (Exception ex) {
            // Ignore
        }
        return DEFAULT;
    }
}
