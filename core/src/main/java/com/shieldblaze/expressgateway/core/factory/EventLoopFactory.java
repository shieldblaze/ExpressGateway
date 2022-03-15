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
package com.shieldblaze.expressgateway.core.factory;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.incubator.channel.uring.IOUringEventLoopGroup;

/**
 * This class provides configured {@link EventLoopGroup} instances.
 */
public final class EventLoopFactory {

    private final EventLoopGroup parentGroup;
    private final EventLoopGroup childGroup;

    /**
     * Create a new {@link EventLoopFactory} instance
     *
     * @param configurationContext {@link ConfigurationContext} instance
     */
    @NonNull
    public EventLoopFactory(ConfigurationContext configurationContext) {
        int parentWorkers = configurationContext.eventLoopConfiguration().parentWorkers();
        int childWorkers = configurationContext.eventLoopConfiguration().childWorkers();

        if (configurationContext.transportConfiguration().transportType() == TransportType.IO_URING) {
            parentGroup = new IOUringEventLoopGroup(parentWorkers);
            childGroup = new IOUringEventLoopGroup(childWorkers);
        } else if (configurationContext.transportConfiguration().transportType() == TransportType.EPOLL) {
            parentGroup = new EpollEventLoopGroup(parentWorkers);
            childGroup = new EpollEventLoopGroup(childWorkers);
        } else {
            parentGroup = new NioEventLoopGroup(parentWorkers);
            childGroup = new NioEventLoopGroup(childWorkers);
        }
    }

    /**
     * Get {@code Parent} {@link EventLoopGroup}
     */
    public EventLoopGroup parentGroup() {
        return parentGroup;
    }

    /**
     * Get {@code Child} {@link EventLoopGroup}
     */
    public EventLoopGroup childGroup() {
        return childGroup;
    }
}
