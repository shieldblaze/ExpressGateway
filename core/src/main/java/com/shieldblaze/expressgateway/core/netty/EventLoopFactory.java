package com.shieldblaze.expressgateway.core.netty;

import io.netty.channel.EventLoopGroup;
import io.netty.channel.epoll.Epoll;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;

public final class EventLoopFactory {
    public static final EventLoopGroup PARENT;
    public static final EventLoopGroup CHILD;

    static {
        if (Epoll.isAvailable()) {
            PARENT = new EpollEventLoopGroup(Runtime.getRuntime().availableProcessors());
            CHILD = new EpollEventLoopGroup(Runtime.getRuntime().availableProcessors() * 2);
        } else {
            PARENT = new NioEventLoopGroup(Runtime.getRuntime().availableProcessors());
            CHILD = new NioEventLoopGroup(Runtime.getRuntime().availableProcessors() * 2);
        }
    }
}
