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
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class ALPNHandlerServer extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNHandlerServer.class);

    private final HTTPConfiguration httpConfiguration;
    private final EventLoopFactory eventLoopFactory;
    private final CommonConfiguration commonConfiguration;
    private final L7Balance l7Balance;
    private final TLSConfiguration tlsConfigurationForClient;

    /**
     * Creates a new instance with the specified fallback protocol name.
     */
    ALPNHandlerServer(L7Balance l7Balance, CommonConfiguration commonConfiguration, TLSConfiguration tlsConfiguration,
                      EventLoopFactory eventLoopFactory, HTTPConfiguration httpConfiguration) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.commonConfiguration = commonConfiguration;
        this.eventLoopFactory = eventLoopFactory;
        this.httpConfiguration = httpConfiguration;
        this.l7Balance = l7Balance;
        this.tlsConfigurationForClient = tlsConfiguration;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        ChannelPipeline pipeline = ctx.pipeline();
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            Http2Connection connection = new DefaultHttp2Connection(true);

            InboundHttp2ToHttpObjectAdapter listener = new InboundHttp2ToHttpObjectAdapterBuilder(connection)
                    .propagateSettings(false)
                    .validateHttpHeaders(true)
                    .maxContentLength(httpConfiguration.getMaxContentLength())
                    .build();

            HttpToHttp2ConnectionHandler http2Handler = new HttpToHttp2ConnectionHandlerBuilder()
                    .frameListener(new HTTP2ContentDecompressor(connection, listener))
                    .connection(connection)
                    .build();

            pipeline.addLast("HTTP2Handler", http2Handler);
//            pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));

            pipeline.addLast("UpstreamHandler", new UpstreamHandler(l7Balance, commonConfiguration, tlsConfigurationForClient,
                    eventLoopFactory, httpConfiguration, true));
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            pipeline.addLast("HTTPServerCodec", HTTPCodecs.newServer(httpConfiguration));
//            pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
//            pipeline.addLast("HTTPContentCompressor", new SmartHTTPContentCompressor(4, 6, 15, 8, 0));
//            pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());

            pipeline.addLast("UpstreamHandler", new UpstreamHandler(l7Balance, commonConfiguration, tlsConfigurationForClient,
                    eventLoopFactory, httpConfiguration));
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
