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

import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.concurrent.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

import java.util.concurrent.Executors;

public class EventStreamConfiguration {

    private final int workers;

    EventStreamConfiguration(int workers) {
        Number.checkZeroOrPositive(workers, "Workers");
        this.workers = workers;
    }

    public static final EventStreamConfiguration DEFAULT = new EventStreamConfiguration(Runtime.getRuntime().availableProcessors() / 2);

    public EventStream newEventStream() {
        EventStream eventStream;
        if (workers == 0) {
            eventStream = new EventStream();
        } else {
            eventStream = new AsyncEventStream(Executors.newFixedThreadPool(workers));
        }
        return eventStream;
    }
}
