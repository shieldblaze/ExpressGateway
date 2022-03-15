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
package com.shieldblaze.expressgateway.configuration.eventloop;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;

/**
 * Configuration for {@link EventLoopConfiguration}.
 *
 * Use {@link EventLoopConfigurationBuilder} to build {@link EventLoopConfiguration} instance.
 */
public final class EventLoopConfiguration implements Configuration {

    @JsonProperty("parentWorkers")
    private int parentWorkers;

    @JsonProperty("childWorkers")
    private int childWorkers;

    EventLoopConfiguration() {
        // Prevent outside initialization
    }

    public static final EventLoopConfiguration DEFAULT = new EventLoopConfiguration();

    private EventLoopConfiguration(int parentWorkers, int childWorkers) {
        this.parentWorkers = parentWorkers;
        this.childWorkers = childWorkers;
    }

    static {
        DEFAULT.parentWorkers = Runtime.getRuntime().availableProcessors();
        DEFAULT.childWorkers = DEFAULT.parentWorkers * 2;
    }

    public int parentWorkers() {
        return parentWorkers;
    }

    EventLoopConfiguration setParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    public int childWorkers() {
        return childWorkers;
    }

    EventLoopConfiguration setChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    public EventLoopConfiguration validate() throws IllegalArgumentException {
        NumberUtil.checkPositive(parentWorkers, "Parent Workers");
        NumberUtil.checkPositive(childWorkers, "Child Workers");
        return this;
    }

    @Override
    public String name() {
        return "EventLoopConfiguration";
    }
}
