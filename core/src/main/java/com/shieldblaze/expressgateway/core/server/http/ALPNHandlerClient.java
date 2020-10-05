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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2FrameListenerDecorator;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.util.concurrent.Promise;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class ALPNHandlerClient extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNHandlerClient.class);

    private final HTTPConfiguration httpConfiguration;
    private final DownstreamHandler downstreamHandler;
    private final Promise<Void> promise;

    ALPNHandlerClient(HTTPConfiguration httpConfiguration, DownstreamHandler downstreamHandler, Promise<Void> promise) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.httpConfiguration = httpConfiguration;
        this.downstreamHandler = downstreamHandler;
        this.promise = promise;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            Http2Connection connection = new DefaultHttp2Connection(false);

            InboundHttp2ToHttpObjectAdapter listener = new InboundHttp2ToHttpObjectAdapterBuilder(connection)
                    .propagateSettings(false)
                    .validateHttpHeaders(true)
                    .maxContentLength((int) httpConfiguration.getMaxContentLength())
                    .build();

            HttpToHttp2ConnectionHandler http2Handler = new HttpToHttp2ConnectionHandlerBuilder()
                    .frameListener(new Http2FrameListenerDecorator(listener))
                    .connection(connection)
                    .build();

            ctx.pipeline().addLast(http2Handler, downstreamHandler);
            promise.trySuccess(null);
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            ctx.pipeline().addLast(
                    new HttpClientCodec(httpConfiguration.getMaxInitialLineLength(), httpConfiguration.getMaxHeaderSize(),
                            httpConfiguration.getMaxChunkSize(), true, true),
                    downstreamHandler
            );
            promise.trySuccess(null);
        } else {
            Throwable throwable = new IllegalArgumentException("Unsupported ALPN Protocol: " + protocol);
            logger.error(throwable);
            promise.tryFailure(throwable);
            ctx.channel().closeFuture();
        }
    }

    Promise<Void> promise() {
        return promise;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at ALPN Client Handler", cause);
    }
}
