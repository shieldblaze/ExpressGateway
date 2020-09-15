package com.shieldblaze.expressgateway.core.netty;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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
