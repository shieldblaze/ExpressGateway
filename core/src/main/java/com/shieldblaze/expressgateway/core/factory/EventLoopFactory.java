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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public final class EventLoopFactory {

    private static final Logger logger = LogManager.getLogger(EventLoopFactory.class);

    private final EventLoopGroup parentGroup;
    private final EventLoopGroup childGroup;

    @NonNull
    public EventLoopFactory(ConfigurationContext configurationContext) {
        int parentWorkers = configurationContext.eventLoopConfiguration().parentWorkers();
        int childWorkers = configurationContext.eventLoopConfiguration().childWorkers();

        String kernelVersion = System.getProperty("os.version", "unknown");
        TransportType transportType = configurationContext.transportConfiguration().transportType();

        record EventLoopPair(EventLoopGroup parent, EventLoopGroup child) {}

        var pair = switch (transportType) {
            case IO_URING -> {
                logger.info("Transport selected: IO_URING (kernel: {})", kernelVersion);
                yield new EventLoopPair(new IOUringEventLoopGroup(parentWorkers), new IOUringEventLoopGroup(childWorkers));
            }
            case EPOLL -> {
                logger.info("Transport selected: EPOLL (kernel: {})", kernelVersion);
                yield new EventLoopPair(new EpollEventLoopGroup(parentWorkers), new EpollEventLoopGroup(childWorkers));
            }
            case NIO -> {
                logger.info("Transport selected: NIO (kernel: {})", kernelVersion);
                logger.warn("NIO transport selected — native transport (epoll/io_uring) recommended for production");
                yield new EventLoopPair(new NioEventLoopGroup(parentWorkers), new NioEventLoopGroup(childWorkers));
            }
        };

        this.parentGroup = pair.parent();
        this.childGroup = pair.child();
    }

    public EventLoopGroup parentGroup() {
        return parentGroup;
    }

    public EventLoopGroup childGroup() {
        return childGroup;
    }
}
