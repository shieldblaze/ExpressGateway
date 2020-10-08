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

import com.shieldblaze.expressgateway.core.concurrent.GlobalEventExecutors;
import com.shieldblaze.expressgateway.core.concurrent.async.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.l4.AbstractL4LoadBalancer;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import com.shieldblaze.expressgateway.core.tls.SNIHandler;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFutureListener;
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

import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

/**
 * TCP Listener for handling incoming requests.
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
     * Create {@link TCPListener} Instance with TLS Server and Client Support (a.k.a TLS Offload and Reload)
     *
     * @param tlsServer {@link TLSConfiguration} for TLS Server
     * @param tlsClient {@link TLSConfiguration} for TLS Client
     */
    public TCPListener(TLSConfiguration tlsServer, TLSConfiguration tlsClient) {
        this.tlsServer = tlsServer;
        this.tlsClient = tlsClient;

        if (tlsServer != null && !tlsServer.isForServer()) {
            throw new IllegalArgumentException("TLSConfiguration for Server is invalid");
        }

        if (tlsClient != null && tlsClient.isForServer()) {
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
    public void start() {

        CommonConfiguration commonConfiguration = getL4LoadBalancer().getCommonConfiguration();
        TransportConfiguration transportConfiguration = commonConfiguration.getTransportConfiguration();
        EventLoopFactory eventLoopFactory = getL4LoadBalancer().getEventLoopFactory();
        ByteBufAllocator byteBufAllocator = getL4LoadBalancer().getByteBufAllocator();

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
                    if (transportConfiguration.getTransportType() == TransportType.EPOLL) {
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
                .childHandler(new ServerInitializer(getL4LoadBalancer(), tlsServer, tlsClient));

        int bindRounds = 1;
        if (transportConfiguration.getTransportType() == TransportType.EPOLL) {
            bindRounds = commonConfiguration.getEventLoopConfiguration().getParentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            CompletableFuture<L4FrontListenerEvent> completableFuture = GlobalEventExecutors.INSTANCE.submitTask(() -> {
                L4FrontListenerEvent l4FrontListenerEvent = new L4FrontListenerEvent();
                try {
                    serverBootstrap.bind(getL4LoadBalancer().getBindAddress()).addListener((ChannelFutureListener) future -> {
                        if (future.isSuccess()) {
                            l4FrontListenerEvent.setChannelFuture(future);
                        } else {
                            l4FrontListenerEvent.setCause(future.cause());
                        }
                    }).sync();
                } catch (InterruptedException e) {
                    l4FrontListenerEvent.setCause(e);
                }
                return l4FrontListenerEvent;
            });

            completableFutureList.add(completableFuture);
        }
    }

    @Override
    public CompletableFuture<Boolean> stop() {
        return GlobalEventExecutors.INSTANCE.submitTask(() -> {
            completableFutureList.forEach(event -> {
                try {
                    event.get().getChannelFuture().channel().close().sync();
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

        private final AbstractL4LoadBalancer abstractL4LoadBalancer;
        private final TLSConfiguration tlsServer;
        private final TLSConfiguration tlsClient;

        ServerInitializer(AbstractL4LoadBalancer abstractL4LoadBalancer, TLSConfiguration tlsServer, TLSConfiguration tlsClient) {
            this.abstractL4LoadBalancer = abstractL4LoadBalancer;
            this.tlsServer = tlsServer;
            this.tlsClient = tlsClient;
        }

        @Override
        protected void initChannel(SocketChannel socketChannel) {
            int timeout = abstractL4LoadBalancer.getCommonConfiguration().getTransportConfiguration().getConnectionIdleTimeout();
            socketChannel.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));

            if (tlsServer != null) {
                socketChannel.pipeline().addLast("SNIHandler", new SNIHandler(tlsServer));
            }

            socketChannel.pipeline().addLast("UpstreamHandler", new UpstreamHandler(abstractL4LoadBalancer, tlsClient));
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ServerInitializer", cause);
        }
    }
}
