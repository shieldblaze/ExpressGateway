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
import com.shieldblaze.expressgateway.configuration.transport.BackendProxyProtocolMode;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolEncoder;
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
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2Settings;
import io.netty.handler.timeout.ReadTimeoutHandler;

import java.time.Duration;
import java.util.concurrent.TimeUnit;

final class Bootstrapper {

    // Backend response timeout is now configurable via HttpConfiguration.backendResponseTimeoutSeconds().

    private final HTTPLoadBalancer httpLoadBalancer;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;

    Bootstrapper(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        eventLoopGroup = httpLoadBalancer.eventLoopFactory().childGroup();
        byteBufAllocator = httpLoadBalancer.byteBufAllocator();
    }

    HttpConnection create(Node node, Channel channel, ConnectionPool pool) {
        // H2-08: Apply configured initial window size to backend connections
        Http2Settings settings = Http2Settings.defaultSettings()
                .initialWindowSize(httpLoadBalancer.httpConfiguration().initialWindowSize());
        return create(node, channel, settings, pool);
    }

    HttpConnection create(Node node, Channel channel, Http2Settings http2Settings, ConnectionPool pool) {
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

                // Backend response timeout: close the connection if the backend goes silent.
                // This is distinct from the idle timeout above — ReadTimeoutHandler fires if
                // *no data at all* is received within the window, protecting against backends
                // that accept a connection but never respond (e.g., stuck in GC, deadlock).
                pipeline.addLast(new ReadTimeoutHandler(httpConfiguration.backendResponseTimeoutSeconds(), TimeUnit.SECONDS));

                // Send PROXY protocol header to backend if configured.
                // Must be before SslHandler so the header is raw TCP bytes
                // preceding any TLS ClientHello.
                BackendProxyProtocolMode ppMode = httpLoadBalancer.configurationContext()
                        .transportConfiguration().backendProxyProtocolMode();
                if (ppMode != BackendProxyProtocolMode.OFF) {
                    pipeline.addLast(new ProxyProtocolEncoder(ppMode, channel));
                }

                if (httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                            .withHTTP2ChannelHandler(CompressibleHttp2FrameCodec
                                    .forClient(httpLoadBalancer.compressionOptions())
                                    .maxHeaderListSize(httpConfiguration.maxHeaderListSize())
                                    .initialSettings(http2Settings)
                                    .build())
                            // RFC 9113 Section 6.9.2: Increase connection-level flow control window
                            // on backend connections to match the configured value.
                            .withHTTP2ChannelHandler(new H2ConnectionWindowHandler(
                                    httpConfiguration.h2ConnectionWindowSize()))
                            .withHTTP2ChannelHandler(new Http2ChannelDuplexHandler() {
                                @Override
                                protected void handlerAdded0(ChannelHandlerContext ctx) throws Exception {
                                    super.handlerAdded0(ctx);
                                }
                            })
                            .withHTTP2ChannelHandler(new DownstreamHandler(httpConnection, channel, pool))
                            .withHTTP1ChannelHandler(new HttpClientCodec(
                                    httpConfiguration.maxInitialLineLength(),
                                    httpConfiguration.maxHeaderSize(),
                                    httpConfiguration.maxChunkSize()
                            ))
                            .withHTTP1ChannelHandler(new HttpContentDecompressor(0))
                            .withHTTP1ChannelHandler(new DownstreamHandler(httpConnection, channel, pool))
                            .build();

                    io.netty.handler.ssl.SslHandler sslHandler = httpLoadBalancer.configurationContext()
                            .tlsClientConfiguration()
                            .defaultMapping()
                            .sslContext()
                            .newHandler(ch.alloc(), node.socketAddress().getHostName(), node.socketAddress().getPort());
                    // RES-01: Explicit TLS handshake timeout for backend connections.
                    sslHandler.setHandshakeTimeoutMillis(10_000);

                    // CF-01: Enable hostname verification to prevent MITM attacks.
                    // Without this, any valid CA-signed certificate is accepted regardless
                    // of whether it matches the backend hostname. The WebSocket bootstrapper
                    // already does this; HTTP bootstrapper was missing it.
                    //
                    // CF-02: Skip hostname verification when acceptAllCerts is enabled.
                    // EndpointIdentificationAlgorithm("HTTPS") is a JDK-level check that
                    // validates the certificate's SAN/CN against the connected hostname,
                    // independent of the trust manager. InsecureTrustManagerFactory (used
                    // when acceptAllCerts=true) only bypasses certificate chain validation,
                    // not hostname verification. When an operator explicitly opts out of
                    // certificate verification (e.g., for self-signed test backends),
                    // hostname verification must also be skipped — otherwise the TLS
                    // handshake fails with a hostname mismatch even though the trust
                    // manager accepted the certificate.
                    if (!httpLoadBalancer.configurationContext().tlsClientConfiguration().acceptAllCerts()) {
                        javax.net.ssl.SSLParameters sslParams = sslHandler.engine().getSSLParameters();
                        sslParams.setEndpointIdentificationAlgorithm("HTTPS");
                        sslHandler.engine().setSSLParameters(sslParams);
                    }

                    pipeline.addLast(sslHandler);
                    pipeline.addLast(alpnHandler);
                } else {
                    pipeline.addLast(new HttpClientCodec(httpConfiguration.maxInitialLineLength(), httpConfiguration.maxHeaderSize(), httpConfiguration.maxChunkSize()));
                    pipeline.addLast(new HttpContentDecompressor(0));
                    pipeline.addLast(new DownstreamHandler(httpConnection, channel, pool));
                }
            }
        });

        // BUG-ALPN-RACE: Set alpnPending BEFORE init() so the happens-before chain
        // guarantees any thread seeing state=CONNECTED_AND_ACTIVE also sees alpnPending=true.
        if (httpLoadBalancer.configurationContext().tlsClientConfiguration().enabled()) {
            httpConnection.setAlpnPending(true);
        }

        ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
        httpConnection.init(channelFuture);
        return httpConnection;
    }
}
