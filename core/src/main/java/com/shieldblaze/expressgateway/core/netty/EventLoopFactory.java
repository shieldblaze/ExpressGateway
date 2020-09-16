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
package com.shieldblaze.expressgateway.core.netty;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;

public final class EventLoopFactory {

    private final EventLoopGroup parentGroup;
    private final EventLoopGroup childGroup;

    public EventLoopFactory(Configuration configuration) {
        if (configuration.getTransportConfiguration().getTransportType() == TransportType.EPOLL) {
            parentGroup = new EpollEventLoopGroup(configuration.getEventLoopConfiguration().getParentWorkers());
            childGroup = new EpollEventLoopGroup(configuration.getEventLoopConfiguration().getChildWorkers());
        } else {
            parentGroup = new NioEventLoopGroup(configuration.getEventLoopConfiguration().getParentWorkers());
            childGroup = new NioEventLoopGroup(configuration.getEventLoopConfiguration().getChildWorkers());
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
