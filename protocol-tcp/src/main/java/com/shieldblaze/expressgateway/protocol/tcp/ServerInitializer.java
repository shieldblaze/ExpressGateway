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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.core.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.SNIHandler;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;

final class ServerInitializer extends ChannelInitializer<SocketChannel> {

    private static final Logger logger = LogManager.getLogger(ServerInitializer.class);

    private final L4LoadBalancer l4LoadBalancer;

    ServerInitializer(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
    }

    @Override
    protected void initChannel(SocketChannel ch) {
        ChannelPipeline pipeline = ch.pipeline();
        pipeline.addLast(l4LoadBalancer.connectionTracker());

        Duration timeout = Duration.ofMillis(l4LoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout());
        pipeline.addLast(new ConnectionTimeoutHandler(timeout, true));

        if (l4LoadBalancer.tlsForServer() != null) {
            pipeline.addLast(new SNIHandler(l4LoadBalancer.tlsForServer()));
        }

        pipeline.addLast(new UpstreamHandler(l4LoadBalancer));
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
