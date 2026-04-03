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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolEncoder;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.ssl.SslHandler;

import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLParameters;
import java.time.Duration;

final class Bootstrapper {
    private final L4LoadBalancer l4LoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        eventLoopGroup = l4LoadBalancer.eventLoopFactory().childGroup();
        byteBufAllocator = l4LoadBalancer.byteBufAllocator();
    }

    TCPConnection newInit(Node node, Channel channel) {
        TCPConnection tcpConnection = new TCPConnection(node);
        tcpConnection.clientChannel(channel); // HIGH-13: Wire client channel for connect failure notification

        Bootstrap bootstrap = BootstrapFactory.tcp(l4LoadBalancer.configurationContext(), eventLoopGroup, byteBufAllocator)
                // RFC 9293 Section 3.6: Enable half-close on backend channels so that
                // a FIN from the backend only shuts down the read side, allowing the
                // proxy to finish flushing data to the client before closing.
                .option(ChannelOption.ALLOW_HALF_CLOSURE, true)
                .handler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        ChannelPipeline pipeline = ch.pipeline();

                        pipeline.addFirst(new NodeBytesTracker(node));

                        Duration timeout = Duration.ofMillis(l4LoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
                        pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                        // Send PROXY protocol header to backend if configured.
                        // Must be before SslHandler so the header is raw TCP bytes
                        // preceding any TLS ClientHello.
                        BackendProxyProtocolMode ppMode = l4LoadBalancer.configurationContext()
                                .transportConfiguration().backendProxyProtocolMode();
                        if (ppMode != BackendProxyProtocolMode.OFF) {
                            pipeline.addLast(new ProxyProtocolEncoder(ppMode, channel));
                        }

                        if (l4LoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
                            String hostname = node.socketAddress().getHostName();
                            int port = node.socketAddress().getPort();
                            SslHandler sslHandler = l4LoadBalancer.configurationContext().tlsClientConfiguration()
                                    .defaultMapping()
                                    .sslContext()
                                    .newHandler(ch.alloc(), hostname, port);

                            // Enable hostname verification to prevent MITM attacks.
                            // Without this, any valid CA-signed certificate is accepted
                            // regardless of whether it matches the backend hostname.
                            //
                            // CF-02: Skip hostname verification when acceptAllCerts is enabled.
                            // See HTTP Bootstrapper CF-02 comment for full rationale.
                            if (!l4LoadBalancer.configurationContext().tlsClientConfiguration().acceptAllCerts()) {
                                SSLEngine engine = sslHandler.engine();
                                SSLParameters params = engine.getSSLParameters();
                                params.setEndpointIdentificationAlgorithm("HTTPS");
                                engine.setSSLParameters(params);
                            }

                            pipeline.addLast(sslHandler);
                        }

                        pipeline.addLast(new DownstreamHandler(channel, node));
                    }
                });

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        tcpConnection.init(channelFuture);
        return tcpConnection;
    }
}
