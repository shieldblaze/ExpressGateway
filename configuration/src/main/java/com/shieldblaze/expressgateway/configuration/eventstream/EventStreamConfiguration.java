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

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;
import java.util.concurrent.Executors;

public class EventStreamConfiguration extends ConfigurationMarshaller {

    private EventStream eventStream;

    @Expose
    private final int workers;

    EventStreamConfiguration(int workers) {
        this.workers = workers;
        init();
    }

    public void init() {
        if (workers == 0) {
            eventStream = new EventStream();
        } else {
            eventStream = new AsyncEventStream(Executors.newFixedThreadPool(workers));
        }
    }

    public int workers() {
        return workers;
    }

    public EventStream eventStream() {
        return eventStream;
    }

    public static EventStreamConfiguration loadFrom(String profileName) throws IOException {
        return loadFrom(EventStreamConfiguration.class, profileName, false, "EventStream.json");
    }

    public void saveTo(String profileName) throws IOException {
        saveTo(this, profileName, false, "EventStream.json");
    }
}
