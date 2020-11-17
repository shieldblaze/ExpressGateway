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
package com.shieldblaze.expressgateway.core.server.tcp;

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.l4.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.core.tls.SNIHandler;
import com.shieldblaze.expressgateway.core.utils.EventLoopFactory;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.epoll.EpollServerSocketChannel;
import io.netty.channel.epoll.EpollServerSocketChannelConfig;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

/**
 * TCP Listener for handling incoming TCP requests.
 */
public final class TCPListener extends L4FrontListener {

    /**
     * Logger
     */
    private static final Logger logger = LogManager.getLogger(TCPListener.class);

    /**
     * {@link TLSConfiguration} for TLS Server Support
     */
    private final TLSConfiguration tlsServer;

    /**
     * {@link TLSConfiguration} for TLS Client Support
     */
    private final TLSConfiguration tlsClient;


    /**
     * Create {@link TCPListener} Instance
     */
    public TCPListener() {
        this(null, null);
    }

    /**
     * Create {@link TCPListener} Instance with TLS Server Support (a.k.a TLS Offload)
     *
     * @param tlsServer {@link TLSConfiguration} for TLS Server
     */
    public TCPListener(TLSConfiguration tlsServer) {
        this(Objects.requireNonNull(tlsServer), null);
    }

    /**
     * Create {@link TCPListener} Instance with TLS Server and Client Support
     *
     * @param tlsServer {@link TLSConfiguration} for TLS Server
     * @param tlsClient {@link TLSConfiguration} for TLS Client
     */
    public TCPListener(TLSConfiguration tlsServer, TLSConfiguration tlsClient) {
        this.tlsServer = tlsServer;
        this.tlsClient = tlsClient;

        if (tlsServer != null && !tlsServer.forServer()) {
            throw new IllegalArgumentException("TLSConfiguration for Server is invalid");
        }

        if (tlsClient != null && tlsClient.forServer()) {
            throw new IllegalArgumentException("TLSConfiguration is Client is invalid");
        }

        if (tlsServer == null) {
            logger.info("TLS Server Support is Disabled");
        }

        if (tlsClient == null) {
            logger.info("TLS Client Support is Disabled");
        }
    }

    @Override
    public List<CompletableFuture<L4FrontListenerEvent>> start() {

        CommonConfiguration commonConfiguration = getL4LoadBalancer().commonConfiguration();
        TransportConfiguration transportConfiguration = commonConfiguration.transportConfiguration();
        EventLoopFactory eventLoopFactory = getL4LoadBalancer().eventLoopFactory();
        ByteBufAllocator byteBufAllocator = getL4LoadBalancer().byteBufAllocator();

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(eventLoopFactory.getParentGroup(), eventLoopFactory.getChildGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfiguration.tcpConnectionBacklog())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
                .childOption(ChannelOption.SO_SNDBUF, transportConfiguration.socketSendBufferSize())
                .childOption(ChannelOption.SO_RCVBUF, transportConfiguration.socketReceiveBufferSize())
                .childOption(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.recvByteBufAllocator())
                .channelFactory(() -> {
                    if (transportConfiguration.transportType() == TransportType.EPOLL) {
                        EpollServerSocketChannel serverSocketChannel = new EpollServerSocketChannel();
                        EpollServerSocketChannelConfig config = serverSocketChannel.config();
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setTcpFastopen(transportConfiguration.tcpFastOpenMaximumPendingRequests());
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);

                        return serverSocketChannel;
                    } else {
                        return new NioServerSocketChannel();
                    }
                })
                .childHandler(new ServerInitializer(getL4LoadBalancer(), tlsServer, tlsClient));

        int bindRounds = 1;
        if (transportConfiguration.transportType() == TransportType.EPOLL) {
            bindRounds = commonConfiguration.eventLoopConfiguration().parentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            CompletableFuture<L4FrontListenerEvent> completableFuture = GlobalExecutors.INSTANCE.submitTask(() -> {
                L4FrontListenerEvent l4FrontListenerEvent = new L4FrontListenerEvent();
                ChannelFuture channelFuture = serverBootstrap.bind(getL4LoadBalancer().bindAddress());
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
            completableFutureList.clear();
            return true;
        });
    }

    private static final class ServerInitializer extends ChannelInitializer<SocketChannel> {

        private static final Logger logger = LogManager.getLogger(ServerInitializer.class);

        private final L4LoadBalancer l4LoadBalancer;
        private final TLSConfiguration tlsServer;
        private final TLSConfiguration tlsClient;

        ServerInitializer(L4LoadBalancer l4LoadBalancer, TLSConfiguration tlsServer, TLSConfiguration tlsClient) {
            this.l4LoadBalancer = l4LoadBalancer;
            this.tlsServer = tlsServer;
            this.tlsClient = tlsClient;
        }

        @Override
        protected void initChannel(SocketChannel socketChannel) {
            int timeout = l4LoadBalancer.commonConfiguration().transportConfiguration().connectionIdleTimeout();
            socketChannel.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));

            if (tlsServer != null) {
                socketChannel.pipeline().addLast(new SNIHandler(tlsServer));
            }

            socketChannel.pipeline().addLast(new UpstreamHandler(l4LoadBalancer, tlsClient));
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ServerInitializer", cause);
        }
    }
}
