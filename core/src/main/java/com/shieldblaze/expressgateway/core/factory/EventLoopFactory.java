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
import io.netty.util.concurrent.DefaultThreadFactory;
import lombok.Getter;
import lombok.experimental.Accessors;
import lombok.extern.log4j.Log4j2;

import java.util.concurrent.TimeUnit;

@Log4j2
@Getter
@Accessors(fluent = true)
public final class EventLoopFactory {

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
                log.info("Transport selected: IO_URING (kernel: {})", kernelVersion);
                yield new EventLoopPair(
                        new IOUringEventLoopGroup(parentWorkers, new DefaultThreadFactory("eg-uring-parent", true)),
                        new IOUringEventLoopGroup(childWorkers, new DefaultThreadFactory("eg-uring-child", true))
                );
            }
            case EPOLL -> {
                log.info("Transport selected: EPOLL (kernel: {})", kernelVersion);
                yield new EventLoopPair(
                        new EpollEventLoopGroup(parentWorkers, new DefaultThreadFactory("eg-epoll-parent", true)),
                        new EpollEventLoopGroup(childWorkers, new DefaultThreadFactory("eg-epoll-child", true))
                );
            }
            case NIO -> {
                log.info("Transport selected: NIO (kernel: {})", kernelVersion);
                log.warn("NIO transport selected — native transport (epoll/io_uring) recommended for production");
                yield new EventLoopPair(
                        new NioEventLoopGroup(parentWorkers, new DefaultThreadFactory("eg-nio-parent", true)),
                        new NioEventLoopGroup(childWorkers, new DefaultThreadFactory("eg-nio-child", true))
                );
            }
        };

        this.parentGroup = pair.parent();
        this.childGroup = pair.child();
    }

    /**
     * Gracefully shut down both event loop groups.
     * Uses a 2-second quiet period and 5-second timeout for in-flight tasks
     * to complete before forcing termination.
     *
     * <p>The quiet period ensures that tasks submitted during shutdown (e.g.,
     * cleanup callbacks from closing channels) have time to execute. Without it,
     * the executor shuts down immediately after the last running task, potentially
     * losing cleanup work.</p>
     */
    public void shutdown() {
        parentGroup.shutdownGracefully(2, 5, TimeUnit.SECONDS);
        childGroup.shutdownGracefully(2, 5, TimeUnit.SECONDS);
    }

    /**
     * Returns {@code true} if both event loop groups have terminated.
     */
    public boolean isShutdown() {
        return parentGroup.isShutdown() && childGroup.isShutdown();
    }

}
