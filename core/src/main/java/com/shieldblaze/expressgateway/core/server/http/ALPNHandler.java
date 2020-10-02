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
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.DelegatingDecompressorFrameListener;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2Settings;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandlerBuilder;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapter;
import io.netty.handler.codec.http2.InboundHttp2ToHttpAdapterBuilder;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class ALPNHandler extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNHandler.class);

    private final HTTPConfiguration httpConfiguration;
    private final EventLoopFactory eventLoopFactory;
    private final CommonConfiguration commonConfiguration;
    private final L7Balance l7Balance;
    private final TLSConfiguration tlsConfigurationForClient;

    /**
     * Creates a new instance with the specified fallback protocol name.
     */
    protected ALPNHandler(L7Balance l7Balance, CommonConfiguration commonConfiguration, TLSConfiguration tlsConfiguration,
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
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            Http2Connection connection = new DefaultHttp2Connection(true);

            InboundHttp2ToHttpAdapter listener = new InboundHttp2ToHttpAdapterBuilder(connection)
                    .propagateSettings(true)
                    .validateHttpHeaders(true)
                    .maxContentLength((int) httpConfiguration.getMaxContentLength())
                    .build();

            Http2Settings http2Settings = new Http2Settings();
            http2Settings.initialWindowSize(httpConfiguration.getInitialWindowSize());
            http2Settings.maxConcurrentStreams(httpConfiguration.getMaxConcurrentStreams());
            http2Settings.maxHeaderListSize(httpConfiguration.getMaxHeaderSizeList());
            http2Settings.pushEnabled(httpConfiguration.enableHTTP2Push());

            HttpToHttp2ConnectionHandler http2Handler = new HttpToHttp2ConnectionHandlerBuilder()
                    .frameListener(new DelegatingDecompressorFrameListener(connection, listener))
                    .connection(connection)
                    .initialSettings(http2Settings)
                    .server(true)
                    .validateHeaders(true)
                    .build();

            ctx.pipeline().addLast(
                    http2Handler,
                    new HttpContentCompressor(),
                    new HttpContentDecompressor(),
                    new HttpServerExpectContinueHandlerImpl(httpConfiguration.getMaxContentLength()),
                    new Handler(l7Balance, commonConfiguration, tlsConfigurationForClient, eventLoopFactory, httpConfiguration)
            );
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            ctx.pipeline().addLast(
                    new HttpClientCodec(),
                    new HttpContentCompressor(),
                    new HttpContentDecompressor(),
                    new HttpServerExpectContinueHandlerImpl(httpConfiguration.getMaxContentLength()),
                    new Handler(l7Balance, commonConfiguration, tlsConfigurationForClient, eventLoopFactory, httpConfiguration)
            );
        } else {
            logger.error("Unsupported ALPN Protocol: {}", protocol);
            ctx.channel().closeFuture();
        }
    }
}
