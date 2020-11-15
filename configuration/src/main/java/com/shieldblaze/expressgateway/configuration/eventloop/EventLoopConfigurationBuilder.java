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
package com.shieldblaze.expressgateway.configuration.eventloop;

import io.netty.util.internal.ObjectUtil;

/**
 * Configuration Builder for {@link EventLoopConfiguration}
 */
public final class EventLoopConfigurationBuilder {
    private int parentWorkers;
    private int childWorkers;

    private EventLoopConfigurationBuilder() {
        // Prevent outside initialization
    }

    /**
     * Create a new {@link EventLoopConfigurationBuilder} Instance
     */
    public static EventLoopConfigurationBuilder newBuilder() {
        return new EventLoopConfigurationBuilder();
    }

    /**
     * Set Number of Parent Workers
     */
    public EventLoopConfigurationBuilder withParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    /**
     * Set Number of Child Workers
     */
    public EventLoopConfigurationBuilder withChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    /**
     * Build {@link EventLoopConfiguration}
     *
     * @return {@link EventLoopConfiguration} Instance
     * @throws IllegalArgumentException If Parent or Child worker count is less than 1
     */
    public EventLoopConfiguration build() {
        return new EventLoopConfiguration()
                .parentWorkers(ObjectUtil.checkPositive(parentWorkers, "Parent Workers"))
                .childWorkers(ObjectUtil.checkPositive(childWorkers, "Child Workers"));
    }
}
