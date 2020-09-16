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
package com.shieldblaze.expressgateway.core.configuration.eventloop;

import io.netty.util.internal.ObjectUtil;

public final class EventLoopConfigurationBuilder {
    private int parentWorkers;
    private int childWorkers;

    private EventLoopConfigurationBuilder() {
    }

    public static EventLoopConfigurationBuilder newBuilder() {
        return new EventLoopConfigurationBuilder();
    }

    public EventLoopConfigurationBuilder withParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    public EventLoopConfigurationBuilder withChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    public EventLoopConfiguration build() {
        EventLoopConfiguration eventLoopConfiguration = new EventLoopConfiguration();
        eventLoopConfiguration.setParentWorkers(ObjectUtil.checkPositive(parentWorkers, "Parent Workers"));
        eventLoopConfiguration.setChildWorkers(ObjectUtil.checkPositive(childWorkers, "Child Workers"));
        return eventLoopConfiguration;
    }
}