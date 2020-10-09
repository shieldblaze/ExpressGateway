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
import io.netty.channel.ChannelPipeline;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import io.netty.util.concurrent.Promise;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * {@link ALPNClientHandler} is used for Application-Layer Protocol Negotiation as client.
 */
final class ALPNClientHandler extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNClientHandler.class);

    private final HTTPConfiguration httpConfiguration;
    private final DownstreamHandler downstreamHandler;
    private final Promise<Void> promise;
    private final boolean isUpstreamHTTP2;

    /**
     * Create a new {@link ALPNClientHandler} Instance
     *
     * @param httpConfiguration {@link HTTPConfiguration} to be applied
     * @param downstreamHandler {@link DownstreamHandler} which will be handling incoming responses
     * @param promise           {@link Promise} to notify when {@link ALPNClientHandler} has finished setting up
     * @param isUpstreamHTTP2   Set to {@code true} if {@link UpstreamHandler} is connected via HTTP/2 else set to {@code false}
     */
    ALPNClientHandler(HTTPConfiguration httpConfiguration, DownstreamHandler downstreamHandler, Promise<Void> promise, boolean isUpstreamHTTP2) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.httpConfiguration = httpConfiguration;
        this.downstreamHandler = downstreamHandler;
        this.promise = promise;
        this.isUpstreamHTTP2 = isUpstreamHTTP2;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        ChannelPipeline pipeline = ctx.pipeline();
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {

            pipeline.addLast("HTTP2Handler", HTTPUtils.h2Handler(httpConfiguration, false));

            // If Upstream is not HTTP/2 then we need HTTPTranslationAdapter for HTTP Message conversion
            if (!isUpstreamHTTP2) {
                pipeline.addLast("HTTPTranslationAdapter", new HTTPTranslationAdapter(false));
            }

            pipeline.addLast("DownstreamHandler", downstreamHandler);
            promise.trySuccess(null);
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            pipeline.addLast("HTTPClientCodec", HTTPUtils.newClientCodec(httpConfiguration));
            pipeline.addLast("HTTPContentCompressor", new HTTPContentCompressor(httpConfiguration));
            pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());

            // If Upstream is HTTP/2 then we need HTTPTranslationAdapter for HTTP Message conversion
            if (isUpstreamHTTP2) {
                pipeline.addLast("HTTPTranslationAdapter", new HTTPTranslationAdapter(true));
            }

            pipeline.addLast("DownstreamHandler", downstreamHandler);
            promise.trySuccess(null);
        } else {
            Throwable throwable = new IllegalArgumentException("Unsupported ALPN Protocol: " + protocol);
            logger.error(throwable);
            promise.tryFailure(throwable);
            ctx.channel().close();
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
