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

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationMarshaller;

import java.io.IOException;

/**
 * Configuration for {@link EventLoopConfiguration}.
 *
 * Use {@link EventLoopConfigurationBuilder} to build {@link EventLoopConfiguration} instance.
 */
public final class EventLoopConfiguration {

    @JsonProperty("parentWorkers")
    private int parentWorkers;

    @JsonProperty("childWorkers")
    private int childWorkers;

    EventLoopConfiguration() {
        // Prevent outside initialization
    }

    public static final EventLoopConfiguration DEFAULT = new EventLoopConfiguration();

    static {
        DEFAULT.parentWorkers = Runtime.getRuntime().availableProcessors();
        DEFAULT.childWorkers = DEFAULT.parentWorkers * 2;
    }

    public int parentWorkers() {
        return parentWorkers;
    }

    EventLoopConfiguration setParentWorkers(int parentWorkers) {
        NumberUtil.checkPositive(parentWorkers, "Parent Workers");
        this.parentWorkers = parentWorkers;
        return this;
    }

    public int childWorkers() {
        return childWorkers;
    }

    EventLoopConfiguration setChildWorkers(int childWorkers) {
        NumberUtil.checkPositive(childWorkers, "Child Workers");
        this.childWorkers = childWorkers;
        return this;
    }

    public EventLoopConfiguration validate() {
        NumberUtil.checkPositive(parentWorkers, "Parent Workers");
        NumberUtil.checkPositive(childWorkers, "Child Workers");
        return this;
    }

    /**
     * Save this configuration to the file
     *
     * @throws IOException If an error occurs during saving
     */
    public void save() throws IOException {
        ConfigurationMarshaller.save("EventLoopConfiguration.json", this);
    }

    /**
     * Load a configuration
     *
     * @return {@link EventLoopConfiguration} Instance
     */
    public static EventLoopConfiguration load() {
        try {
            return ConfigurationMarshaller.load("EventLoopConfiguration.json", EventLoopConfiguration.class);
        } catch (Exception ex) {
            // Ignore
        }
        return DEFAULT;
    }
}
