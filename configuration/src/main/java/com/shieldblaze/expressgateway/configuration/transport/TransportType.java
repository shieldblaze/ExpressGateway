/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.transport;

import io.netty.channel.epoll.EpollDatagramChannel;
import io.netty.channel.epoll.EpollEventLoopGroup;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.nio.NioDatagramChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;
import io.netty.incubator.channel.uring.IOUringDatagramChannel;
import io.netty.incubator.channel.uring.IOUringEventLoopGroup;
import io.netty.incubator.channel.uring.IOUringServerSocketChannel;
import io.netty.incubator.channel.uring.IOUringSocketChannel;

public enum TransportType {

    /**
     * <p> This uses Java NIO (Fast) </p>
     * Uses:
     * <ul>
     *     <li> {@link NioSocketChannel} </li>
     *     <li> {@link NioServerSocketChannel} </li>
     *     <li> {@link NioDatagramChannel} </li>
     *     <li> {@link NioEventLoopGroup} </li>
     * </ul>
     */
    NIO,
    /**
     * <p> This uses Linux Epoll (Faster) </p>
     * Uses:
     * <ul>
     *     <li> {@link EpollSocketChannel} </li>
     *     <li> {@link EpollServerSocketChannel} </li>
     *     <li> {@link EpollDatagramChannel} </li>
     *     <li> {@link EpollEventLoopGroup} </li>
     * </ul>
     */
    EPOLL,
    /**
     * <p> This uses Linux IO_URING (Fastest) </p>
     * Uses:
     * <ul>
     *     <li> {@link IOUringSocketChannel} </li>
     *     <li> {@link IOUringServerSocketChannel} </li>
     *     <li> {@link IOUringDatagramChannel} </li>
     *     <li> {@link IOUringEventLoopGroup} </li>
     * </ul>
     */
    IO_URING
}
