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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.websocketx.WebSocketClientProtocolHandler;
import io.netty.handler.ssl.SslHandler;

import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLParameters;
import java.time.Duration;

import static io.netty.handler.codec.http.websocketx.WebSocketVersion.V13;

final class Bootstrapper {

    private final HTTPLoadBalancer httpLoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        eventLoopGroup = httpLoadBalancer.eventLoopFactory().childGroup();
        byteBufAllocator = httpLoadBalancer.byteBufAllocator();
    }

    WebSocketConnection newInit(Node node, WebSocketUpgradeProperty wsProperty) {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(Headers.X_FORWARDED_FOR, wsProperty.clientAddress().getAddress().getHostAddress()); // Add Client IP Address
        WebSocketConnection connection = new WebSocketConnection(node);

        Bootstrap bootstrap = BootstrapFactory.tcp(httpLoadBalancer.configurationContext(), eventLoopGroup, byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                // Add NodeBytesTracker
                pipeline.addFirst(new NodeBytesTracker(node));

                // Add ConnectionTimeoutHandler
                Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
                pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                // If TLS is enabled then add TLS Handler
                if (httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
                    String hostname = node.socketAddress().getHostName();
                    int port = node.socketAddress().getPort();
                    SslHandler sslHandler = httpLoadBalancer.configurationContext().tlsClientConfiguration()
                            .defaultMapping()
                            .sslContext()
                            .newHandler(ch.alloc(), hostname, port);

                    // Enable hostname verification to prevent MITM attacks.
                    // Without this, any valid CA-signed certificate is accepted
                    // regardless of whether it matches the backend hostname.
                    //
                    // CF-02: Skip hostname verification when acceptAllCerts is enabled.
                    // See HTTP Bootstrapper CF-02 comment for full rationale.
                    if (!httpLoadBalancer.configurationContext().tlsClientConfiguration().acceptAllCerts()) {
                        SSLEngine engine = sslHandler.engine();
                        SSLParameters params = engine.getSSLParameters();
                        params.setEndpointIdentificationAlgorithm("HTTPS");
                        engine.setSSLParameters(params);
                    }

                    pipeline.addLast(sslHandler);
                }

                // Add HTTP Client
                pipeline.addLast(new HttpClientCodec(
                        httpLoadBalancer.httpConfiguration().maxInitialLineLength(),
                        httpLoadBalancer.httpConfiguration().maxHeaderSize(),
                        httpLoadBalancer.httpConfiguration().maxChunkSize()
                ));

                // Add HTTP Object Aggregator to aggregate HTTP Objects
                // MED-15: Fixed from 8196 (typo for 8192, too small) to 65536
                pipeline.addLast(new HttpObjectAggregator(65536));

                // Add WebSocket protocol handler which manages the handshaking process.
                pipeline.addLast(new WebSocketClientProtocolHandler(wsProperty.uri(), V13, wsProperty.subProtocol(), true, headers, 65536));

                // Add WebSocket close handshake (RFC 6455 Section 7) and idle timeout
                // handlers before the downstream handler.
                pipeline.addLast("ws-downstream", new WebSocketDownstreamHandler(wsProperty.channel()));
                WebSocketPipelineUtils.addWebSocketHandlers(pipeline, "ws-downstream");
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        connection.init(channelFuture);
        return connection;
    }
}
