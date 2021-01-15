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
package com.shieldblaze.expressgateway.configuration.eventloop;

import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;

/**
 * {@code EventLoop} Configuration
 */
public final class EventLoopConfiguration extends ConfigurationMarshaller {

    @Expose
    private int parentWorkers;

    @Expose
    private int childWorkers;

    EventLoopConfiguration() {
        // Prevent outside initialization
    }

    public int parentWorkers() {
        return parentWorkers;
    }

    EventLoopConfiguration parentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    public int childWorkers() {
        return childWorkers;
    }

    EventLoopConfiguration childWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    public static EventLoopConfiguration loadFrom(String profileName) throws IOException {
        return loadFrom(EventLoopConfiguration.class, profileName, false, "EventLoop.json");
    }

    public void saveTo(String profileName) throws IOException {
        saveTo(this, profileName, false, "EventLoop.json");
    }
}
