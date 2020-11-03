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
package com.shieldblaze.expressgateway.backend.connection;

import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.EventLoopGroup;

import java.util.Objects;

/**
 * Netty {@link Bootstrap} initializer
 */
public abstract class Bootstrapper {

    private CommonConfiguration commonConfiguration;
    private EventLoopGroup eventLoopGroup;
    private ByteBufAllocator allocator;

    /**
     * Create a new {@link Bootstrap} Instance for new connection.
     */
    public abstract Bootstrap bootstrap();

    public void setEventLoopFactory(EventLoopGroup eventLoopGroup) {
        if (this.eventLoopGroup == null) {
            this.eventLoopGroup = Objects.requireNonNull(eventLoopGroup, "EventLoopGroup");
        } else {
            throw new IllegalArgumentException("EventLoopGroup is already set");
        }
    }

    protected CommonConfiguration getCommonConfiguration() {
        return commonConfiguration;
    }

    public void setCommonConfiguration(CommonConfiguration commonConfiguration) {
        if (this.commonConfiguration == null) {
            this.commonConfiguration = Objects.requireNonNull(commonConfiguration, "CommonConfiguration");
        } else {
            throw new IllegalArgumentException("CommonConfiguration is already set");
        }
    }

    protected EventLoopGroup getEventLoopGroup() {
        return eventLoopGroup;
    }

    protected ByteBufAllocator getAllocator() {
        return allocator;
    }

    public void setAllocator(ByteBufAllocator allocator) {
        if (this.allocator == null) {
            this.allocator = Objects.requireNonNull(allocator, "ByteBufAllocator");
        } else {
            throw new IllegalArgumentException("ByteBufAllocator is already set");
        }
    }
}
