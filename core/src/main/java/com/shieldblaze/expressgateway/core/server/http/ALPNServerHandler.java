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

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * {@link ALPNServerHandler} is used for Application-Layer Protocol Negotiation as client.
 */
final class ALPNServerHandler extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNServerHandler.class);

    final HTTPConfiguration httpConfiguration;
    final EventLoopFactory eventLoopFactory;
    final CommonConfiguration commonConfiguration;
    final L7Balance l7Balance;
    final TLSConfiguration tlsConfiguration;

    /**
     * Creates a new instance with the specified fallback protocol name.
     */
    ALPNServerHandler(HTTPListener.ServerInitializer serverInitializer) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.l7Balance = serverInitializer.l7Balance;
        this.commonConfiguration = serverInitializer.commonConfiguration;
        this.tlsConfiguration = serverInitializer.tlsConfigurationForClient;
        this.eventLoopFactory = serverInitializer.eventLoopFactory;
        this.httpConfiguration = serverInitializer.httpConfiguration;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        ChannelPipeline pipeline = ctx.pipeline();
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            pipeline.addLast("HTTP2Handler", HTTPUtils.h2Handler(httpConfiguration, true));
            pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
            pipeline.addLast("UpstreamHandler", new UpstreamHandler(this, true));
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            pipeline.addLast("HTTPServerCodec", HTTPCodecs.newServer(httpConfiguration));
            pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
            pipeline.addLast("UpstreamHandler", new UpstreamHandler(this, false));
        } else {
            if (logger.isErrorEnabled()) {
                Throwable throwable = new IllegalArgumentException("Unsupported ALPN Protocol: " + protocol);
                logger.error(throwable);
            }
            ctx.channel().closeFuture();
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
//        logger.error("Caught Error at ALPN Server Handler", cause);
    }
}
