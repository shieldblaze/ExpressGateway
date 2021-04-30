/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.core.BootstrapFactory;
import com.shieldblaze.expressgateway.core.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.HTTPCodecs;
import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.websocketx.WebSocket13FrameDecoder;
import io.netty.handler.codec.http.websocketx.WebSocketClientHandshaker;
import io.netty.handler.codec.http.websocketx.WebSocketClientHandshakerFactory;
import io.netty.handler.codec.http.websocketx.extensions.compression.WebSocketClientCompressionHandler;
import io.netty.handler.ssl.SslHandler;

import java.net.URI;
import java.time.Duration;

import static io.netty.handler.codec.http.websocketx.WebSocketVersion.V13;

final class Bootstrapper {

    private final HTTPLoadBalancer httpLoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.eventLoopGroup = httpLoadBalancer.eventLoopFactory().childGroup();
        this.byteBufAllocator = httpLoadBalancer.byteBufAllocator();
    }

    WebSocketConnection newInit(Node node, WebSocketUpgradeProperty wsProperty) {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(Headers.X_FORWARDED_FOR, wsProperty.clientAddress().getAddress().getHostAddress());
        WebSocketClientHandshaker factory = WebSocketClientHandshakerFactory.newHandshaker(wsProperty.uri(), V13, wsProperty.subProtocol(), true, headers);
        WebSocketConnection connection = new WebSocketConnection(node, factory);

        Bootstrap bootstrap = BootstrapFactory.tcp(httpLoadBalancer.coreConfiguration(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                // Add NodeBytesTracker
                pipeline.addFirst(new NodeBytesTracker(node));

                // Add ConnectionTimeoutHandler
                Duration timeout = Duration.ofMillis(httpLoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout());
                pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                // If TLS is enabled then add TLS Handler
                if (httpLoadBalancer.tlsForClient() != null) {
                    String hostname = node.socketAddress().getHostName();
                    int port = node.socketAddress().getPort();
                    SslHandler sslHandler = httpLoadBalancer.tlsForClient()
                            .defaultMapping()
                            .sslContext()
                            .newHandler(ch.alloc(), hostname, port);

                    pipeline.addLast(sslHandler);
                }

                // Add HTTP Client
                pipeline.addLast(HTTPCodecs.client(httpLoadBalancer.httpConfiguration()));

                // Add HTTP Object Aggregator to aggregate HTTP Objects
                pipeline.addLast(new HttpObjectAggregator(8196));

                // Add WebSocketClientHandshakerFinisherHandler which will finish the
                // handshaking process.
                pipeline.addLast(new WebSocketClientHandshakerFinisherHandler(factory));

                // Add Downstream Handler
                pipeline.addLast(new WebSocketDownstreamHandler(wsProperty.channel()));
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        connection.init(channelFuture);
        return connection;
    }
}
