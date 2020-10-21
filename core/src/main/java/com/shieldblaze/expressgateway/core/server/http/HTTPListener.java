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

import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.core.server.http.compression.HTTPContentCompressor;
import com.shieldblaze.expressgateway.core.server.http.compression.HTTPContentDecompressor;
import com.shieldblaze.expressgateway.core.tls.SNIHandler;
import com.shieldblaze.expressgateway.core.utils.EventLoopFactory;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
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
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

/**
 * HTTP Listener for for handling incoming HTTP requests.
 */
public final class HTTPListener extends HTTPFrontListener {

    @Override
    public List<CompletableFuture<L4FrontListenerEvent>> start() {
        CommonConfiguration commonConfiguration = getL7LoadBalancer().getCommonConfiguration();
        TransportConfiguration transportConfiguration = commonConfiguration.getTransportConfiguration();
        EventLoopFactory eventLoopFactory = getL7LoadBalancer().getEventLoopFactory();
        ByteBufAllocator byteBufAllocator = getL7LoadBalancer().getByteBufAllocator();

        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(eventLoopFactory.getParentGroup(), eventLoopFactory.getChildGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, transportConfiguration.getRecvByteBufAllocator())
                .option(ChannelOption.SO_RCVBUF, transportConfiguration.getSocketReceiveBufferSize())
                .option(ChannelOption.SO_BACKLOG, transportConfiguration.getTCPConnectionBacklog())
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, true)
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
                .childHandler(new ServerInitializer((HTTPLoadBalancer) getL7LoadBalancer()));

        int bindRounds = 1;
        if (transportConfiguration.getTransportType() == TransportType.EPOLL) {
            bindRounds = commonConfiguration.getEventLoopConfiguration().getParentWorkers();
        }

        for (int i = 0; i < bindRounds; i++) {
            CompletableFuture<L4FrontListenerEvent> completableFuture = GlobalExecutors.INSTANCE.submitTask(() -> {
                L4FrontListenerEvent l4FrontListenerEvent = new L4FrontListenerEvent();
                try {
                    serverBootstrap.bind(getL7LoadBalancer().getBindAddress()).addListener((ChannelFutureListener) future -> {
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

        return completableFutureList;
    }

    @Override
    public CompletableFuture<Boolean> stop() {
        return GlobalExecutors.INSTANCE.submitTask(() -> {
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

        final HTTPLoadBalancer httpLoadBalancer;

        ServerInitializer(HTTPLoadBalancer httpLoadBalancer) {
            this.httpLoadBalancer = httpLoadBalancer;
        }

        @Override
        protected void initChannel(SocketChannel socketChannel) {
            ChannelPipeline pipeline = socketChannel.pipeline();
            HTTPConfiguration httpConfiguration = httpLoadBalancer.getHTTPConfiguration();

            int timeout = httpLoadBalancer.getCommonConfiguration().getTransportConfiguration().getConnectionIdleTimeout();
            pipeline.addFirst("IdleStateHandler", new IdleStateHandler(timeout, timeout, timeout));

            // If TLS Server is not enabled then we'll only use HTTP/1.1
            if (httpLoadBalancer.getTlsServer() == null) {
                pipeline.addLast("HTTPServerCodec", HTTPUtils.newServerCodec(httpConfiguration));
                pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
                pipeline.addLast("HTTPContentCompressor", new HTTPContentCompressor(httpConfiguration));
                pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());
                pipeline.addLast("UpstreamHandler", new UpstreamHandler(httpLoadBalancer));
            } else {
                pipeline.addLast("SNIHandler", new SNIHandler(httpLoadBalancer.getTlsServer()));

                ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                        // HTTP/2 Handlers
                        .withHTTP2ChannelHandler("HTTP2Handler", HTTPUtils.serverH2Handler(httpConfiguration))
                        .withHTTP2ChannelHandler("HTTP2MultiplexHandler", new Http2MultiplexHandler(new MultiplexInitializer(httpLoadBalancer)))
                        // HTTP/1.1 Handlers
                        .withHTTP1ChannelHandler("HTTPServerCodec", HTTPUtils.newServerCodec(httpConfiguration))
                        .withHTTP1ChannelHandler("HTTPServerValidator", new HTTPServerValidator(httpConfiguration))
                        .withHTTP1ChannelHandler("HTTPContentCompressor", new HTTPContentCompressor(httpConfiguration))
                        .withHTTP1ChannelHandler("HTTPContentDecompressor", new HTTPContentDecompressor())
                        .withHTTP1ChannelHandler("UpstreamHandler", new UpstreamHandler(httpLoadBalancer))
                        .build();

                pipeline.addLast("ALPNHandler", alpnHandler);
            }
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ServerInitializer", cause);
        }
    }

    private static final class MultiplexInitializer extends ChannelInitializer<Channel> {

        private final HTTPLoadBalancer httpLoadBalancer;

        private MultiplexInitializer(HTTPLoadBalancer httpLoadBalancer) {
            this.httpLoadBalancer = httpLoadBalancer;
        }

        @Override
        protected void initChannel(Channel ch) {
            ChannelPipeline pipeline = ch.pipeline();
            pipeline.addLast("DuplexHTTP2ToHTTPObjectAdapter", new DuplexHTTP2ToHTTPObjectAdapter());
            pipeline.addLast("UpstreamHandler", new UpstreamHandler(httpLoadBalancer, true));
        }
    }
}
