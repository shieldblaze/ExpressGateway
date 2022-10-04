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

import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.handlers.SNIHandler;
import com.shieldblaze.expressgateway.metrics.EdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2InboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPContentCompressor;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpServerKeepAliveHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.time.Duration;

public final class DefaultHTTPServerInitializer extends HTTPServerInitializer {

    private static final Logger logger = LogManager.getLogger(DefaultHTTPServerInitializer.class);

    @Override
    protected void initChannel(SocketChannel socketChannel) {
        ChannelPipeline pipeline = socketChannel.pipeline();
        pipeline.addFirst(EdgeNetworkMetricRecorder.INSTANCE);
        pipeline.addLast(httpLoadBalancer.connectionTracker());

        Duration timeout = Duration.ofMillis(httpLoadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
        pipeline.addLast(new ConnectionTimeoutHandler(timeout, true));

        // If TLS Server is not enabled then we'll only use HTTP/1.1
        HttpConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();
        if (httpLoadBalancer.configurationContext().tlsServerConfiguration().enabled()) {
            pipeline.addLast(new SNIHandler(httpLoadBalancer.configurationContext().tlsServerConfiguration()));

            ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                    // HTTP/2 Handlers
                    .withHTTP2ChannelHandler(HTTPCodecs.http2ServerCodec(httpConfiguration))
                    .withHTTP2ChannelHandler(new HTTP2InboundAdapter())
                    .withHTTP2ChannelHandler(new UpstreamHandler(httpLoadBalancer, true))

                    // HTTP/1.1 Handlers
                    .withHTTP1ChannelHandler(HTTPCodecs.http1ServerCodec(httpConfiguration))
                    .withHTTP1ChannelHandler(new HttpServerKeepAliveHandler())
                    .withHTTP1ChannelHandler(new HTTPServerValidator(httpConfiguration))
                    .withHTTP1ChannelHandler(new HTTPContentCompressor(httpConfiguration))
                    .withHTTP1ChannelHandler(new HttpContentDecompressor())
                    .withHTTP1ChannelHandler(new UpstreamHandler(httpLoadBalancer, true))
                    .build();

            pipeline.addLast(alpnHandler);
        } else {
            pipeline.addLast(HTTPCodecs.http1ServerCodec(httpConfiguration));
            pipeline.addLast(new HttpServerKeepAliveHandler());
            pipeline.addLast(new HTTPServerValidator(httpConfiguration));
            pipeline.addLast(new HTTPContentCompressor(httpConfiguration));
            pipeline.addLast(new HttpContentDecompressor());
            pipeline.addLast(new UpstreamHandler(httpLoadBalancer, false));
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
