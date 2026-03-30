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
package com.shieldblaze.expressgateway.protocol.http.loadbalancer;

import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolHandler;
import com.shieldblaze.expressgateway.core.handlers.SNIHandler;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.CompressibleHttp2FrameCodec;
import com.shieldblaze.expressgateway.protocol.http.AccessLogHandler;
import com.shieldblaze.expressgateway.protocol.http.H2ConnectionWindowHandler;
import com.shieldblaze.expressgateway.protocol.http.H2cHandler;
import com.shieldblaze.expressgateway.protocol.http.Http11ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.Http2ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.HttpServerInitializer;
import com.shieldblaze.expressgateway.protocol.http.RequestBodySizeLimitHandler;
import com.shieldblaze.expressgateway.protocol.http.RequestBodyTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.RequestHeaderTimeoutHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.compression.Http11CorrectContentCompressor;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpDecoderConfig;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpServerExpectContinueHandler;
import io.netty.handler.codec.http.HttpServerKeepAliveHandler;
import io.netty.handler.codec.http2.Http2Settings;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;

final class StandardHttpServerInitializer extends HttpServerInitializer {

    private static final Logger logger = LogManager.getLogger(StandardHttpServerInitializer.class);

    @Override
    protected void initChannel(SocketChannel socketChannel) {
        ChannelPipeline pipeline = socketChannel.pipeline();
        pipeline.addFirst(StandardEdgeNetworkMetricRecorder.INSTANCE);
        pipeline.addLast(httpLoadBalancer.connectionTracker());

        // Add PROXY protocol handler if enabled
        ProxyProtocolMode proxyMode = httpLoadBalancer.configurationContext().transportConfiguration().proxyProtocolMode();
        if (proxyMode != null && proxyMode != ProxyProtocolMode.OFF) {
            pipeline.addLast(new ProxyProtocolHandler(proxyMode));
        }

        Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
        pipeline.addLast(new ConnectionTimeoutHandler(timeout, true));

        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();
        if (httpLoadBalancer.configurationContext().tlsServerConfiguration().enabled()) {
            // RFC 9113 Section 6.5.2: Configure SETTINGS_MAX_CONCURRENT_STREAMS
            // to limit the number of streams a client can open concurrently.
            // H2-08: Configure SETTINGS_INITIAL_WINDOW_SIZE to control per-stream
            // flow control window, balancing throughput vs memory consumption.
            Http2Settings http2Settings = Http2Settings.defaultSettings()
                    .maxConcurrentStreams(httpConfiguration.maxConcurrentStreams())
                    .initialWindowSize(httpConfiguration.initialWindowSize());

            // TLS enabled: use ALPN to negotiate HTTP/2 or HTTP/1.1
            ALPNHandlerBuilder alpnBuilder = ALPNHandlerBuilder.newBuilder()
                    .withHTTP2ChannelHandler(CompressibleHttp2FrameCodec.forServer(httpLoadBalancer.compressionOptions())
                            .maxHeaderListSize(httpConfiguration.maxHeaderListSize())
                            .initialSettings(http2Settings)
                            .build())
                    // RFC 9113 Section 6.9.2: Increase connection-level flow control window
                    // from the default 65535 to the configured value. This handler sends a
                    // WINDOW_UPDATE on stream 0 and then removes itself from the pipeline.
                    .withHTTP2ChannelHandler(new H2ConnectionWindowHandler(httpConfiguration.h2ConnectionWindowSize()))
                    .withHTTP2ChannelHandler(new Http2ServerInboundHandler(httpLoadBalancer, true))
                    .withHTTP1ChannelHandler(new HttpServerCodec(new HttpDecoderConfig()
                            .setMaxInitialLineLength(httpConfiguration.maxInitialLineLength())
                            .setMaxHeaderSize(httpConfiguration.maxHeaderSize())
                            .setMaxChunkSize(httpConfiguration.maxChunkSize())
                            .setValidateHeaders(true)
                    ))
                    // SEC-02: Slowloris defense — enforce a deadline for receiving the first
                    // complete set of request headers. Must be after HttpServerCodec (which
                    // decodes bytes into HttpRequest objects) and before the business-logic
                    // handlers. Once headers arrive, the handler removes itself.
                    .withHTTP1ChannelHandler(new RequestHeaderTimeoutHandler(httpConfiguration.requestHeaderTimeoutSeconds()))
                    // H1-03: RFC 9110 Section 10.1.1 — Handle "Expect: 100-continue" before
                    // the request body is read. Netty's handler automatically sends 100 Continue
                    // or rejects with 417 Expectation Failed as appropriate.
                    .withHTTP1ChannelHandler(new HttpServerExpectContinueHandler())
                    .withHTTP1ChannelHandler(new HttpServerKeepAliveHandler())
                    .withHTTP1ChannelHandler(new Http11CorrectContentCompressor(httpConfiguration.compressionThreshold(), httpLoadBalancer.compressionOptions()))
                    .withHTTP1ChannelHandler(new HttpContentDecompressor(0))
                    .withHTTP1ChannelHandler(new RequestBodySizeLimitHandler(httpConfiguration.maxRequestBodySize()));

            // ME-04: Slow-POST defense — enforce a deadline for receiving the complete
            // request body. Disabled when requestBodyTimeoutSeconds is 0.
            if (httpConfiguration.requestBodyTimeoutSeconds() > 0) {
                alpnBuilder.withHTTP1ChannelHandler(new RequestBodyTimeoutHandler(httpConfiguration.requestBodyTimeoutSeconds()));
            }

            ALPNHandler alpnHandler = alpnBuilder
                    .withHTTP1ChannelHandler(new AccessLogHandler())
                    .withHTTP1ChannelHandler(new Http11ServerInboundHandler(httpLoadBalancer, true))
                    .build();

            pipeline.addLast(new SNIHandler(httpLoadBalancer.configurationContext().tlsServerConfiguration()));
            pipeline.addLast(alpnHandler);
        } else {
            // TLS disabled: Support HTTP/1.1 + h2c (HTTP/2 cleartext) per RFC 9113 Sec 3.2-3.4.
            //
            // H2cHandler (ByteToMessageDecoder) detects HTTP/2 prior knowledge by reading
            // the first 24 bytes and checking for the HTTP/2 connection preface. If detected,
            // it replaces the pipeline with H2 codec + H2 handler. Otherwise, it installs
            // the standard HTTP/1.1 pipeline.
            //
            // h2c upgrade via "Upgrade: h2c" header is handled in Http11ServerInboundHandler
            // alongside WebSocket upgrade detection, avoiding the conflict caused by
            // HttpServerUpgradeHandler intercepting ALL upgrade requests (including WebSocket).
            pipeline.addLast(new H2cHandler(httpLoadBalancer));
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
