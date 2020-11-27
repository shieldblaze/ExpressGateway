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

import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.SelfSignedCertificate;

public class HTTPServer extends Thread {

    private final int port;
    private final boolean tls;
    private EventLoopGroup eventLoopGroup;
    private ChannelFuture channelFuture;

    public HTTPServer(int port, boolean tls) {
        this.port = port;
        this.tls = tls;
    }

    @Override
    public void run() {

        try {

            eventLoopGroup = new NioEventLoopGroup(2);

            SelfSignedCertificate selfSignedCertificate = new SelfSignedCertificate("localhost", "EC", 256);
            SslContext sslContext = SslContextBuilder.forServer(selfSignedCertificate.certificate(), selfSignedCertificate.privateKey())
                    .sslProvider(SslProvider.JDK)
                    .applicationProtocolConfig(new ApplicationProtocolConfig(
                            ApplicationProtocolConfig.Protocol.ALPN,
                            ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                            ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                            ApplicationProtocolNames.HTTP_2,
                            ApplicationProtocolNames.HTTP_1_1))
                    .build();

            ServerBootstrap serverBootstrap = new ServerBootstrap()
                    .group(eventLoopGroup, eventLoopGroup)
                    .channel(NioServerSocketChannel.class)
                    .childHandler(new ChannelInitializer<SocketChannel>() {
                        @Override
                        protected void initChannel(SocketChannel ch) {

                            if (tls) {
                                ch.pipeline().addLast(sslContext.newHandler(ch.alloc()));

                                Http2Connection http2Connection = new DefaultHttp2Connection(true);

                                InboundHttp2ToHttpAdapter adapter = new InboundHttp2ToHttpAdapterBuilder(http2Connection)
                                        .propagateSettings(false)
                                        .maxContentLength(Integer.MAX_VALUE)
                                        .validateHttpHeaders(true)
                                        .build();

                                HttpToHttp2ConnectionHandler httpToHttp2ConnectionHandler = new HttpToHttp2ConnectionHandlerBuilder()
                                        .connection(http2Connection)
                                        .frameListener(adapter)
                                        .build();

                                ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                                        .withHTTP2ChannelHandler(httpToHttp2ConnectionHandler)
                                        .withHTTP1ChannelHandler(new HttpServerCodec())
                                        .withHTTP1ChannelHandler(new HttpObjectAggregator(Integer.MAX_VALUE))
                                        .withHTTP1ChannelHandler(new Handler())
                                        .build();

                                ch.pipeline().addLast(alpnHandler);
                            } else {
                                ch.pipeline().addLast(new HttpServerCodec());
                                ch.pipeline().addLast(new HttpObjectAggregator(Integer.MAX_VALUE));
                                ch.pipeline().addLast(new Handler());
                            }
                        }
                    });

            channelFuture = serverBootstrap.bind(port);
        } catch (Exception ex) {
            ex.printStackTrace();
        }
    }

    public void shutdown() {
        channelFuture.channel().close();
        eventLoopGroup.shutdownGracefully();
    }

    private static final class Handler extends SimpleChannelInboundHandler<FullHttpRequest> {

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
            httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, 4);
            ctx.writeAndFlush(httpResponse);
        }
    }
}