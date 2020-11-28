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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.adapter.http1.HTTPOutboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2OutboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPContentDecompressor;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.ssl.SslHandler;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class Bootstrapper {

    private static final Logger logger = LogManager.getLogger(Bootstrapper.class);

    private final HTTPLoadBalancer httpLoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(HTTPLoadBalancer httpLoadBalancer, EventLoopGroup eventLoopGroup, ByteBufAllocator byteBufAllocator) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.eventLoopGroup = eventLoopGroup;
        this.byteBufAllocator = byteBufAllocator;
    }

    HTTPConnection newInit(Node node, Channel channel) {
        int timeout = httpLoadBalancer.coreConfiguration().transportConfiguration().backendConnectTimeout();
        HTTPConnection httpConnection = new HTTPConnection(node, timeout);

        Bootstrap bootstrap = BootstrapFactory.getTCP(httpLoadBalancer.coreConfiguration(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                int timeout = httpLoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout();
                pipeline.addFirst(new IdleStateHandler(timeout, timeout, timeout));

                DownstreamHandler downstreamHandler = new DownstreamHandler(httpConnection, channel);
                httpConnection.downstreamHandler(downstreamHandler);

                if (httpLoadBalancer.tlsForClient() == null) {
                    pipeline.addLast(HTTPUtils.newClientCodec(httpLoadBalancer.httpConfiguration()));
                    pipeline.addLast(new HTTPContentDecompressor());
                    pipeline.addLast(new HTTPOutboundAdapter());
                    pipeline.addLast(downstreamHandler);

                    try {
                        httpConnection.lease();
                    } catch (IllegalAccessException e) {
                        logger.error(e);
                    }
                } else {
                    String hostname = node.socketAddress().getHostName();
                    int port = node.socketAddress().getPort();
                    SslHandler sslHandler = httpLoadBalancer.tlsForClient()
                            .defaultMapping()
                            .sslContext()
                            .newHandler(ch.alloc(), hostname, port);

                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                            // HTTP/2 Handlers
                            .withHTTP2ChannelHandler(HTTPUtils.clientH2Handler(httpLoadBalancer.httpConfiguration()))
                            .withHTTP2ChannelHandler(new HTTP2OutboundAdapter())
                            .withHTTP2ChannelHandler(downstreamHandler)
                            // HTTP/1.1 Handlers
                            .withHTTP1ChannelHandler(HTTPUtils.newClientCodec(httpLoadBalancer.httpConfiguration()))
                            .withHTTP1ChannelHandler(new HTTPContentDecompressor())
                            .withHTTP1ChannelHandler(new HTTPOutboundAdapter())
                            .withHTTP1ChannelHandler(downstreamHandler)
                            .build();

                    pipeline.addLast(sslHandler);
                    pipeline.addLast(alpnHandler);
                }
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        httpConnection.init(channelFuture);

        channelFuture.addListener((ChannelFutureListener) future -> {
           if (!future.isSuccess()) {
               channel.close();
           }
        });

        return httpConnection;
    }
}
