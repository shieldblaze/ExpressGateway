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

import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
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
import io.netty.handler.codec.http2.HttpConversionUtil;
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

import java.net.InetSocketAddress;
import java.util.Objects;

public class HTTPServer extends Thread {

    private int port;
    private final boolean tls;
    private final ChannelHandler channelHandler;
    private EventLoopGroup eventLoopGroup;
    private ChannelFuture channelFuture;

    public HTTPServer(boolean tls, ChannelHandler channelHandler) {
        this.tls = tls;
        this.channelHandler = Objects.requireNonNullElseGet(channelHandler, Handler::new);
    }

    @Override
    public void run() {
        try {
            eventLoopGroup = new NioEventLoopGroup(Runtime.getRuntime().availableProcessors());

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
                                        .withHTTP2ChannelHandler(channelHandler)
                                        .withHTTP1ChannelHandler(new HttpServerCodec())
                                        .withHTTP1ChannelHandler(new HttpObjectAggregator(Integer.MAX_VALUE))
                                        .withHTTP1ChannelHandler(channelHandler)
                                        .build();

                                ch.pipeline().addLast(alpnHandler);
                            } else {
                                ch.pipeline().addLast(new HttpServerCodec());
                                ch.pipeline().addLast(new HttpObjectAggregator(Integer.MAX_VALUE));
                                ch.pipeline().addLast(channelHandler);
                            }
                        }
                    });

            channelFuture = serverBootstrap.bind("127.0.0.1", 0).sync();
            port = ((InetSocketAddress) channelFuture.channel().localAddress()).getPort();
        } catch (Exception ex) {
            ex.printStackTrace();
        }
    }

    public int port() {
        return port;
    }

    public void shutdown() {
        channelFuture.channel().close();
        eventLoopGroup.shutdownGracefully();
    }
}
