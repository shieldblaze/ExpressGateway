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
package com.shieldblaze.expressgateway.protocol.http.alpn;

import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;

/**
 * {@linkplain ALPNHandler} handles Application Layer Protocol Negotiation.
 */
public final class ALPNHandler extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNHandler.class);

    private Map<String, ChannelHandler> http1ChannelHandlerMap;
    private Map<String, ChannelHandler> http2ChannelHandlerMap;
    private final CompletableFuture<String> ALPNProtocol;

    /**
     * Create a new {@link ALPNHandler} Instance
     *
     * @param http1ChannelHandlerMap Handles to be added when ALPN negotiates with HTTP/1.1
     * @param http2ChannelHandlerMap Handles to be added when ALPN negotiates with HTTP/2
     */
    ALPNHandler(Map<String, ChannelHandler> http1ChannelHandlerMap, Map<String, ChannelHandler> http2ChannelHandlerMap) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.http1ChannelHandlerMap = Objects.requireNonNull(http1ChannelHandlerMap, "HTTP1ChannelHandlerMap");
        this.http2ChannelHandlerMap = Objects.requireNonNull(http2ChannelHandlerMap, "HTTP2ChannelHandlerMap");
        ALPNProtocol = new CompletableFuture<>();
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        ChannelPipeline pipeline = ctx.pipeline();

        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {

            // Add all channel handlers from HTTP/2 map to pipeline
            for (Map.Entry<String, ChannelHandler> entry : http2ChannelHandlerMap.entrySet()) {
                pipeline.addLast(entry.getKey(), entry.getValue());
            }

            complete(ApplicationProtocolNames.HTTP_2);
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {

            // Add all channel handlers from HTTP/1.1 map to pipeline
            for (Map.Entry<String, ChannelHandler> entry : http1ChannelHandlerMap.entrySet()) {
                pipeline.addLast(entry.getKey(), entry.getValue());
            }

            complete(ApplicationProtocolNames.HTTP_1_1);
        } else {
            IllegalArgumentException exception = new IllegalArgumentException("Unsupported ALPN Protocol: " + protocol);
            ALPNProtocol.completeExceptionally(exception);
            logger.error(exception);
            ctx.channel().close();
        }
    }

    private void complete(String protocol) {
        ALPNProtocol.complete(protocol);
        http1ChannelHandlerMap = null;
        http2ChannelHandlerMap = null;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at ALPN Handler", cause);
    }

    /**
     * Get the negotiated protocol
     */
    public CompletableFuture<String> protocol() {
        return ALPNProtocol;
    }
}
