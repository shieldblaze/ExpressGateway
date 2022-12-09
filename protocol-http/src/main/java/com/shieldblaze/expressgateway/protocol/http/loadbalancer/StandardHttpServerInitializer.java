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
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.SNIHandler;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.CompressibleHttp2FrameCodec;
import com.shieldblaze.expressgateway.protocol.http.Http11ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.Http2ServerInboundHandler;
import com.shieldblaze.expressgateway.protocol.http.HttpServerInitializer;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.compression.Http11CorrectContentCompressor;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpServerKeepAliveHandler;
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

        Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
        pipeline.addLast(new ConnectionTimeoutHandler(timeout, true));

        // If TLS Server is not enabled then we'll only use HTTP/1.1
        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();
        if (httpLoadBalancer.configurationContext().tlsServerConfiguration().enabled()) {
            ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                    .withHTTP2ChannelHandler(CompressibleHttp2FrameCodec.forServer(httpLoadBalancer.compressionOptions()).build())
                    .withHTTP2ChannelHandler(new Http2ServerInboundHandler(httpLoadBalancer, true))
                    .withHTTP1ChannelHandler(new HttpServerCodec(
                            httpConfiguration.maxInitialLineLength(),
                            httpConfiguration.maxHeaderSize(),
                            httpConfiguration.maxChunkSize(),
                            true
                    ))
                    .withHTTP1ChannelHandler(new HttpServerKeepAliveHandler())
                    .withHTTP1ChannelHandler(new Http11CorrectContentCompressor(httpConfiguration.compressionThreshold(), httpLoadBalancer.compressionOptions()))
                    .withHTTP1ChannelHandler(new HttpContentDecompressor())
                    .withHTTP1ChannelHandler(new Http11ServerInboundHandler(httpLoadBalancer, true))
                    .build();

            pipeline.addLast(new SNIHandler(httpLoadBalancer.configurationContext().tlsServerConfiguration()));
            pipeline.addLast(alpnHandler);
        } else {
            pipeline.addLast(new HttpServerCodec(
                            httpConfiguration.maxInitialLineLength(),
                            httpConfiguration.maxHeaderSize(),
                            httpConfiguration.maxChunkSize(),
                            true
                    ))
                    .addLast(new HttpServerKeepAliveHandler())
                    .addLast(new Http11CorrectContentCompressor(httpConfiguration.compressionThreshold(), httpLoadBalancer.compressionOptions()))
                    .addLast(new HttpContentDecompressor())
                    .addLast(new Http11ServerInboundHandler(httpLoadBalancer, false));
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
