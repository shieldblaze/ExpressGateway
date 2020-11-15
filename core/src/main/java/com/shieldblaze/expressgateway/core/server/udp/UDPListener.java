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

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.utils.BootstrapFactory;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.EventLoopGroup;

import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

/**
 * UDP Listener for handling incoming UDP requests.
 */
public final class UDPListener extends L4FrontListener {

    @Override
    public List<CompletableFuture<L4FrontListenerEvent>> start() {
        CommonConfiguration commonConfiguration = getL4LoadBalancer().getCommonConfiguration();
        EventLoopGroup eventLoopGroup = getL4LoadBalancer().getEventLoopFactory().getParentGroup();

        Bootstrap bootstrap = BootstrapFactory.getUDP(commonConfiguration, eventLoopGroup, getL4LoadBalancer().getByteBufAllocator())
                .handler(new UpstreamHandler(getL4LoadBalancer()));

        int bindRounds = 1;
        if (commonConfiguration.getTransportConfiguration().getTransportType() == TransportType.EPOLL) {
            bindRounds = commonConfiguration.getEventLoopConfiguration().getParentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            CompletableFuture<L4FrontListenerEvent> completableFuture = GlobalExecutors.INSTANCE.submitTask(() -> {
                L4FrontListenerEvent l4FrontListenerEvent = new L4FrontListenerEvent();
                ChannelFuture channelFuture = bootstrap.bind(getL4LoadBalancer().getBindAddress());
                l4FrontListenerEvent.channelFuture(channelFuture);
                return l4FrontListenerEvent;
            });

            completableFutureList.add(completableFuture);
        }

        return completableFutureList;
    }

    @Override
    public CompletableFuture<Boolean> stop() {
        return GlobalExecutors.INSTANCE.submitTask(() -> {
            completableFutureList.forEach(event -> {
                try {
                    event.get().channelFuture().channel().close().sync();
                } catch (InterruptedException | ExecutionException e) {
                    // Ignore
                }
            });
            return true;
        }).whenComplete((result, throwable) -> completableFutureList.clear());
    }
}
