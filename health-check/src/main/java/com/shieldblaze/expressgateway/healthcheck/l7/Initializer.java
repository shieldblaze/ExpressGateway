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
package com.shieldblaze.expressgateway.healthcheck.l7;

import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.DelegatingDecompressorFrameListener;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.timeout.IdleStateHandler;
import io.netty.util.concurrent.DefaultPromise;
import io.netty.util.concurrent.Promise;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class Initializer extends ChannelInitializer<SocketChannel> {

    private final SslContext sslContext;
    private final HTTPHealthCheck httpHealthCheck;

    Initializer(SslContext sslContext, HTTPHealthCheck httpHealthCheck) {
        this.sslContext = sslContext;
        this.httpHealthCheck = httpHealthCheck;
    }

    @Override
    protected void initChannel(SocketChannel ch) {
        ch.pipeline().addLast(new IdleStateHandler(httpHealthCheck.timeout, httpHealthCheck.timeout, httpHealthCheck.timeout))
                .addLast(sslContext.newHandler(ch.alloc(), httpHealthCheck.url.getHost(), httpHealthCheck.url.getPort()))
                .addLast(new ALPNHandler(new DefaultPromise<>(ch.eventLoop())));
    }

    static final class ALPNHandler extends ApplicationProtocolNegotiationHandler {

        private static final Logger logger = LogManager.getLogger(ALPNHandler.class);
        private static final int MAX_CONTENT_LENGTH = 1024000;
        private final Promise<String> promise;

        protected ALPNHandler(Promise<String> promise) {
            super(ApplicationProtocolNames.HTTP_1_1);
            this.promise = promise;
        }

        @Override
        protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
            if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                Http2Connection connection = new DefaultHttp2Connection(false);

                InboundHttp2ToHttpAdapter listener = new InboundHttp2ToHttpAdapterBuilder(connection)
                        .propagateSettings(true)
                        .validateHttpHeaders(true)
                        .maxContentLength(MAX_CONTENT_LENGTH)
                        .build();

                HttpToHttp2ConnectionHandler http2Handler = new HttpToHttp2ConnectionHandlerBuilder()
                        .frameListener(new DelegatingDecompressorFrameListener(connection, listener))
                        .connection(connection)
                        .build();

                ctx.pipeline().addLast(http2Handler,
                        new HttpObjectAggregator(MAX_CONTENT_LENGTH, true),
                        new Handler(new DefaultPromise<>(ctx.executor())));

                promise.setSuccess(ApplicationProtocolNames.HTTP_2);
            } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
                ctx.pipeline().addLast(
                        new HttpClientCodec(),
                        new HttpObjectAggregator(MAX_CONTENT_LENGTH, true),
                        new Handler(new DefaultPromise<>(ctx.channel().eventLoop())));

                promise.setSuccess(ApplicationProtocolNames.HTTP_1_1);
            } else {
                throw new IllegalArgumentException("Unknown Protocol: " + protocol);
            }
        }

        @Override
        protected void handshakeFailure(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ALPN Handler", cause);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Caught Error At ALPN Handler", cause);
        }

        Promise<String> getPromise() {
            return promise;
        }
    }
}
