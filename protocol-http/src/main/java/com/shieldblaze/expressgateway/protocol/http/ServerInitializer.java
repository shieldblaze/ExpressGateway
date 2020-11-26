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

import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.SNIHandler;
import com.shieldblaze.expressgateway.protocol.http.adapter.http1.HTTPInboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2InboundAdapter;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPContentCompressor;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPContentDecompressor;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class ServerInitializer extends HTTPServerInitializer {

    private static final Logger logger = LogManager.getLogger(ServerInitializer.class);

    @Override
    protected void initChannel(SocketChannel socketChannel) {
        ChannelPipeline pipeline = socketChannel.pipeline();
        HTTPConfiguration httpConfiguration = httpLoadBalancer.httpConfiguration();

        int timeout = httpLoadBalancer.coreConfiguration().transportConfiguration().connectionIdleTimeout();
        pipeline.addFirst(new IdleStateHandler(timeout, timeout, timeout));

        // If TLS Server is not enabled then we'll only use HTTP/1.1
        if (httpLoadBalancer.tlsForServer() == null) {
            pipeline.addLast(HTTPUtils.newServerCodec(httpConfiguration));
            pipeline.addLast(new HTTPServerValidator(httpConfiguration));
            pipeline.addLast(new HTTPContentCompressor(httpConfiguration));
            pipeline.addLast(new HTTPContentDecompressor());
            pipeline.addLast(new HTTPInboundAdapter());
            pipeline.addLast(new UpstreamHandler(httpLoadBalancer));
        } else {
            pipeline.addLast(new SNIHandler(httpLoadBalancer.tlsForServer()));

            ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                    // HTTP/2 Handlers
                    .withHTTP2ChannelHandler(HTTPUtils.serverH2Handler(httpConfiguration))
                    .withHTTP2ChannelHandler(new HTTP2InboundAdapter())
                    .withHTTP2ChannelHandler(new UpstreamHandler(httpLoadBalancer))

                    // HTTP/1.1 Handlers
                    .withHTTP1ChannelHandler(HTTPUtils.newServerCodec(httpConfiguration))
                    .withHTTP1ChannelHandler(new HTTPServerValidator(httpConfiguration))
                    .withHTTP1ChannelHandler(new HTTPContentCompressor(httpConfiguration))
                    .withHTTP1ChannelHandler(new HTTPContentDecompressor())
                    .withHTTP1ChannelHandler(new HTTPInboundAdapter())
                    .withHTTP1ChannelHandler(new UpstreamHandler(httpLoadBalancer))
                    .build();

            pipeline.addLast(alpnHandler);
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error At ServerInitializer", cause);
    }
}
