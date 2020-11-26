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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.socket.DatagramChannel;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

public class ServerInitializer extends ChannelInitializer<DatagramChannel> {

    private static final Logger logger = LogManager.getLogger(ServerInitializer.class);

    private final L4LoadBalancer l4LoadBalancer;
    private final ChannelHandler channelHandler;

    public ServerInitializer(L4LoadBalancer l4LoadBalancer, ChannelHandler channelHandler) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.channelHandler = channelHandler;
    }

    @Override
    protected void initChannel(DatagramChannel datagramChannel) {
        int timeout = l4LoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout();
        datagramChannel.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));
        datagramChannel.pipeline().addLast(channelHandler);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
