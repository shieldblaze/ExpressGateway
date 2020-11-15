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
package com.shieldblaze.expressgateway.core.utils;

import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;

public final class EventLoopFactory {

    private final EventLoopGroup parentGroup;
    private final EventLoopGroup childGroup;

    public EventLoopFactory(CommonConfiguration commonConfiguration) {
        if (commonConfiguration.transportConfiguration().transportType() == TransportType.EPOLL) {
            parentGroup = new EpollEventLoopGroup(commonConfiguration.eventLoopConfiguration().parentWorkers());
            childGroup = new EpollEventLoopGroup(commonConfiguration.eventLoopConfiguration().childWorkers());
        } else {
            parentGroup = new NioEventLoopGroup(commonConfiguration.eventLoopConfiguration().parentWorkers());
            childGroup = new NioEventLoopGroup(commonConfiguration.eventLoopConfiguration().childWorkers());
        }
    }

    /**
     * Get {@code Parent} {@link EventLoopGroup}
     */
    public EventLoopGroup getParentGroup() {
        return parentGroup;
    }

    /**
     * Get {@code Child} {@link EventLoopGroup}
     */
    public EventLoopGroup getChildGroup() {
        return childGroup;
    }
}
