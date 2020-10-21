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
package com.shieldblaze.expressgateway.core.server.http.pool;

import com.shieldblaze.expressgateway.backend.connection.Bootstrapper;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.ChannelOption;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollSocketChannel;
import io.netty.channel.socket.nio.NioSocketChannel;

public class HTTPBootstrapper extends Bootstrapper {

    @Override
    public Bootstrap bootstrap() {
        TransportConfiguration transportConfiguration = getCommonConfiguration().getTransportConfiguration();

        return new Bootstrap()
                .group(getEventLoopGroup())
                .option(ChannelOption.ALLOCATOR, getAllocator())
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.getRecvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, transportConfiguration.getSocketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.getSocketReceiveBufferSize())
                .option(ChannelOption.TCP_NODELAY, true)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
                .option(ChannelOption.CONNECT_TIMEOUT_MILLIS, transportConfiguration.getBackendConnectTimeout())
                .channelFactory(() -> {
                    if (transportConfiguration.getTransportType() == TransportType.EPOLL) {
                        EpollSocketChannel socketChannel = new EpollSocketChannel();
                        socketChannel.config()
                                .setEpollMode(EpollMode.EDGE_TRIGGERED)
                                .setTcpFastOpenConnect(true)
                                .setTcpQuickAck(true);

                        return socketChannel;
                    } else {
                        return new NioSocketChannel();
                    }
                });
    }
}
