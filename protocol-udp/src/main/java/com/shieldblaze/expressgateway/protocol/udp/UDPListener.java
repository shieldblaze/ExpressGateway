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

import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopEvent;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandler;
import io.netty.channel.EventLoopGroup;

import java.util.ArrayList;
import java.util.List;

/**
 * UDP Listener for handling incoming UDP requests.
 */
public class UDPListener extends L4FrontListener {

    private final List<ChannelFuture> channelFutures = new ArrayList<>();

    @Override
    public L4FrontListenerStartupEvent start() {
        L4FrontListenerStartupEvent l4FrontListenerStartupEvent = new L4FrontListenerStartupEvent();

        // If ChannelFutureList is not 0 then this listener is already started and we won't start it again.
        if (channelFutures.size() != 0) {
            l4FrontListenerStartupEvent.tryFailure(new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        CoreConfiguration coreConfiguration = l4LoadBalancer().coreConfiguration();
        EventLoopGroup eventLoopGroup = l4LoadBalancer().eventLoopFactory().parentGroup();

        ChannelHandler channelHandler;
        if (l4LoadBalancer().channelHandler() == null) {
            channelHandler = new UpstreamHandler(l4LoadBalancer());
        } else {
            channelHandler = l4LoadBalancer().channelHandler();
        }

        Bootstrap bootstrap = BootstrapFactory.getUDP(coreConfiguration, eventLoopGroup, l4LoadBalancer().byteBufAllocator())
                .handler(channelHandler);

        int bindRounds = 1;
        if (coreConfiguration.transportConfiguration().transportType() == TransportType.EPOLL ||
                coreConfiguration.transportConfiguration().transportType() == TransportType.IO_URING) {
            bindRounds = coreConfiguration.eventLoopConfiguration().parentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = bootstrap.bind(l4LoadBalancer().bindAddress());
            channelFutures.add(channelFuture);
        }

        // Add listener to last ChannelFuture to notify all listeners
        channelFutures.get(channelFutures.size() - 1).addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                l4FrontListenerStartupEvent.trySuccess(null);
            } else {
                l4FrontListenerStartupEvent.tryFailure(future.cause());
            }
        });

        l4LoadBalancer().publishEvent(l4FrontListenerStartupEvent);
        return l4FrontListenerStartupEvent;
    }

    @Override
    public L4FrontListenerStopEvent stop() {
        L4FrontListenerStopEvent l4FrontListenerStopEvent = new L4FrontListenerStopEvent();

        channelFutures.forEach(channelFuture -> channelFuture.channel().close());
        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                l4FrontListenerStopEvent.trySuccess(null);
            } else {
                l4FrontListenerStopEvent.tryFailure(future.cause());
            }
        });

        l4LoadBalancer().publishEvent(l4FrontListenerStopEvent);
        return l4FrontListenerStopEvent;
    }
}
