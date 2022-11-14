/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2Settings;

import java.time.Duration;

final class Bootstrapper {

    private final HTTPLoadBalancer httpLoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.eventLoopGroup = httpLoadBalancer.eventLoopFactory().childGroup();
        this.byteBufAllocator = httpLoadBalancer.byteBufAllocator();
    }

    HttpConnection create(Node node, Channel channel) {
        return create(node, channel, Http2Settings.defaultSettings());
    }

    HttpConnection create(Node node, Channel channel, Http2Settings http2Settings) {
        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();
        HttpConnection httpConnection = new HttpConnection(node, httpConfiguration);

        Bootstrap bootstrap = BootstrapFactory.tcp(httpLoadBalancer.configurationContext(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                pipeline.addFirst(new NodeBytesTracker(node));

                Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
                pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                if (httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                            .withHTTP2ChannelHandler(CompressibleHttp2FrameCodec.forClient(httpLoadBalancer.compressionOptions()).initialSettings(http2Settings).build())
                            .withHTTP2ChannelHandler(new Http2ChannelDuplexHandler() {
                                @Override
                                protected void handlerAdded0(ChannelHandlerContext ctx) throws Exception {
                                    super.handlerAdded0(ctx);
                                }
                            })
                            .withHTTP2ChannelHandler(new DownstreamHandler(httpConnection, channel))
                            .withHTTP1ChannelHandler(new HttpServerCodec(
                                    httpConfiguration.maxInitialLineLength(),
                                    httpConfiguration.maxHeaderSize(),
                                    httpConfiguration.maxChunkSize(),
                                    true
                            ))
                            .withHTTP1ChannelHandler(new HttpContentDecompressor())
                            .withHTTP1ChannelHandler(new DownstreamHandler(httpConnection, channel))
                            .build();

                    pipeline.addLast(httpLoadBalancer.configurationContext().tlsClientConfiguration()
                            .defaultMapping()
                            .sslContext()
                            .newHandler(ch.alloc(), node.socketAddress().getHostName(), node.socketAddress().getPort()));
                    pipeline.addLast(alpnHandler);
                } else {
                    pipeline.addLast(new HttpClientCodec(httpConfiguration.maxInitialLineLength(), httpConfiguration.maxHeaderSize(), httpConfiguration.maxChunkSize()));
                    pipeline.addLast(new HttpContentDecompressor());
                    pipeline.addLast(new DownstreamHandler(httpConnection, channel));
                }
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        httpConnection.init(channelFuture);
        return httpConnection;
    }
}
