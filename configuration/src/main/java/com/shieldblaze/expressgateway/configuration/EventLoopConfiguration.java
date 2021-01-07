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

/**
 * {@code EventLoop} Configuration
 */
public final class EventLoopConfiguration implements Configuration {

    public static final EventLoopConfiguration EMPTY_INSTANCE = new EventLoopConfiguration();

    @Expose
    @JsonProperty("parentWorkers")
    private int parentWorkers;

    @Expose
    @JsonProperty("childWorkers")
    private int childWorkers;

    public int parentWorkers() {
        return parentWorkers;
    }

    public EventLoopConfiguration parentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    public int childWorkers() {
        return childWorkers;
    }

    public EventLoopConfiguration childWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    @Override
    public String name() {
        return "EventLoop";
    }

    @Override
    public void validate() throws IllegalArgumentException {
        Number.checkPositive(parentWorkers, "Parent Workers");
        Number.checkPositive(childWorkers, "Child Workers");
    }
}
