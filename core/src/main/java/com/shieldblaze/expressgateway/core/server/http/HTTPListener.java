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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.server.L7FrontListener;
import com.shieldblaze.expressgateway.core.tls.SNIHandler;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollServerSocketChannelConfig;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

public final class HTTPListener extends L7FrontListener {

    /**
     * Logger
     */
    private static final Logger logger = LogManager.getLogger(HTTPListener.class);

    /**
     * {@link TLSConfiguration} for TLS Server Support
     */
    private final TLSConfiguration tlsConfigurationForServer;

    /**
     * {@link TLSConfiguration} for TLS Client Support
     */
    private final TLSConfiguration tlsConfigurationForClient;

    /**
     * Create {@link HTTPListener} Instance
     *
     * @param bindAddress {@link InetSocketAddress} on which {@link HTTPListener} will bind and listen.
     */
    public HTTPListener(InetSocketAddress bindAddress) {
        this(bindAddress, null);
    }

    /**
     * Create {@link HTTPListener} Instance with TLS Server Support (a.k.a TLS Offload)
     *
     * @param bindAddress               {@link InetSocketAddress} on which {@link HTTPListener} will bind and listen.
     * @param tlsConfigurationForServer {@link TLSConfiguration} for TLS Server
     */
    public HTTPListener(InetSocketAddress bindAddress, TLSConfiguration tlsConfigurationForServer) {
        this(bindAddress, tlsConfigurationForServer, null);
    }

    /**
     * Create {@link HTTPListener} Instance with TLS Server and Client Support (a.k.a TLS Offload and Reload)
     *
     * @param bindAddress               {@link InetSocketAddress} on which {@link HTTPListener} will bind and listen.
     * @param tlsConfigurationForServer {@link TLSConfiguration} for TLS Server
     * @param tlsConfigurationForClient {@link TLSConfiguration} for TLS Client
     */
    public HTTPListener(InetSocketAddress bindAddress, TLSConfiguration tlsConfigurationForServer, TLSConfiguration tlsConfigurationForClient) {
        super(bindAddress);
        this.tlsConfigurationForServer = tlsConfigurationForServer;
        this.tlsConfigurationForClient = tlsConfigurationForClient;

        if (tlsConfigurationForServer != null && !tlsConfigurationForServer.isForServer()) {
            throw new IllegalArgumentException("TLSConfiguration for Server is invalid");
        }

        if (tlsConfigurationForClient != null && tlsConfigurationForClient.isForServer()) {
            throw new IllegalArgumentException("TLSConfiguration is Client is invalid");
        }

        if (tlsConfigurationForServer == null) {
            logger.info("TLS Server Support is Disabled");
        }

        if (tlsConfigurationForClient == null) {
            logger.info("TLS Client Support is Disabled");
        }
    }

    @Override
    public void start(CommonConfiguration commonConfiguration, EventLoopFactory eventLoopFactory, ByteBufAllocator byteBufAllocator,
                      HTTPConfiguration httpConfiguration, L7Balance l7Balance) {

        TransportConfiguration transportConfiguration = commonConfiguration.getTransportConfiguration();

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(eventLoopFactory.getParentGroup(), eventLoopFactory.getChildGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.getRecvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.getSocketReceiveBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfiguration.getTCPConnectionBacklog())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .childOption(ChannelOption.SO_SNDBUF, transportConfiguration.getSocketSendBufferSize())
                .childOption(ChannelOption.SO_RCVBUF, transportConfiguration.getSocketReceiveBufferSize())
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.getRecvByteBufAllocator())
                .channelFactory(() -> {
                    if (commonConfiguration.getTransportConfiguration().getTransportType() == TransportType.EPOLL) {
                        EpollServerSocketChannel serverSocketChannel = new EpollServerSocketChannel();
                        EpollServerSocketChannelConfig config = serverSocketChannel.config();
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setTcpFastopen(transportConfiguration.getTCPFastOpenMaximumPendingRequests());
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);

                        return serverSocketChannel;
                    } else {
                        return new NioServerSocketChannel();
                    }
                })
                .childHandler(new ServerInitializer(httpConfiguration, commonConfiguration, eventLoopFactory, l7Balance,
                        tlsConfigurationForServer, tlsConfigurationForClient));

        int bindRounds = 1;
        if (transportConfiguration.getTransportType() == TransportType.EPOLL) {
            bindRounds = commonConfiguration.getEventLoopConfiguration().getParentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = serverBootstrap.bind(bindAddress).addListener((ChannelFutureListener) future -> {
                if (future.isSuccess()) {
                    logger.info("Server Successfully Started at: {}", future.channel().localAddress());
                }
            });

            channelFutureList.add(channelFuture);
        }
    }

    private static final class ServerInitializer extends ChannelInitializer<SocketChannel> {

        private static final Logger logger = LogManager.getLogger(ServerInitializer.class);

        private final HTTPConfiguration httpConfiguration;
        private final EventLoopFactory eventLoopFactory;
        private final CommonConfiguration commonConfiguration;
        private final L7Balance l7Balance;
        private final TLSConfiguration tlsConfigurationForServer;
        private final TLSConfiguration tlsConfigurationForClient;

        ServerInitializer(HTTPConfiguration httpConfiguration, CommonConfiguration commonConfiguration, EventLoopFactory eventLoopFactory,
                          L7Balance l7Balance, TLSConfiguration tlsConfigurationForServer, TLSConfiguration tlsConfigurationForClient) {
            this.httpConfiguration = httpConfiguration;
            this.commonConfiguration = commonConfiguration;
            this.eventLoopFactory = eventLoopFactory;
            this.l7Balance = l7Balance;
            this.tlsConfigurationForServer = tlsConfigurationForServer;
            this.tlsConfigurationForClient = tlsConfigurationForClient;
        }

        @Override
        protected void initChannel(SocketChannel socketChannel) {
            ChannelPipeline pipeline = socketChannel.pipeline();

            int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();
            pipeline.addFirst("IdleStateHandler", new IdleStateHandler(timeout, timeout, timeout));

            // If TLS is not enabled then we'll only use HTTP/1.1
            if (tlsConfigurationForServer == null) {
                pipeline.addLast("HTTPServerCodec", HTTPCodecs.newServer(httpConfiguration));
                pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
                pipeline.addLast("UpstreamHandler", new UpstreamHandler(l7Balance, commonConfiguration, tlsConfigurationForClient,
                        eventLoopFactory, httpConfiguration));
            } else {
                pipeline.addLast("SNIHandler", new SNIHandler(tlsConfigurationForServer));
                pipeline.addLast("ALPNServerHandler", new ALPNHandlerServer(l7Balance, commonConfiguration, tlsConfigurationForClient,
                        eventLoopFactory, httpConfiguration));
            }
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ServerInitializer", cause);
        }
    }
}
