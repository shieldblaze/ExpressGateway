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
package com.shieldblaze.expressgateway.core.server.udp;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.BootstrapFactory;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.server.FrontListener;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

/**
 * UDP Listener for handling incoming requests.
 */
public final class UDPListener extends FrontListener {
    private static final Logger logger = LogManager.getLogger(UDPListener.class);

    /**
     * @param bindAddress {@link InetSocketAddress} on which {@link UDPListener} will bind and listen.
     */
    public UDPListener(InetSocketAddress bindAddress) {
        super(bindAddress);
    }

    @Override
    public void start(Configuration configuration, EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator, L4Balance l4Balance) {

        int bindRounds = 1;
        if (configuration.getTransportConfiguration().getTransportType() == TransportType.EPOLL) {
            bindRounds = configuration.getEventLoopConfiguration().getParentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            Bootstrap bootstrap = BootstrapFactory.getUDP(configuration, eventLoopFactory.getParentGroup(), byteBufAllocator)
                    .handler(new UpstreamHandler(configuration, eventLoopFactory, l4Balance));

            ChannelFuture channelFuture = bootstrap.bind(bindAddress).addListener((ChannelFutureListener) future -> {
                if (future.isSuccess()) {
                    logger.info("Server Successfully Started at: {}", future.channel().localAddress());
                }
            });

            channelFutureList.add(channelFuture);
        }
    }
}
