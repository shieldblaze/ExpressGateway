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
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.handler.codec.http2.Http2MultiplexHandler;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.ApplicationProtocolNegotiationHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * {@link ALPNServerHandler} is used for Application-Layer Protocol Negotiation as client.
 */
final class ALPNServerHandler extends ApplicationProtocolNegotiationHandler {

    private static final Logger logger = LogManager.getLogger(ALPNServerHandler.class);

    final HTTPLoadBalancer httpLoadBalancer;
    final TLSConfiguration tlsClient;

    /**
     * Create a new {@link ALPNServerHandler} Instance
     *
     * @param httpLoadBalancer {@link HTTPLoadBalancer} Instance
     * @param tlsClient        {@link TLSConfiguration} Instance if we'll use TLS when connecting
     *                         to backend else set to {@code null}
     */
    ALPNServerHandler(HTTPLoadBalancer httpLoadBalancer, TLSConfiguration tlsClient) {
        super(ApplicationProtocolNames.HTTP_1_1);
        this.httpLoadBalancer = httpLoadBalancer;
        this.tlsClient = tlsClient;
    }

    @Override
    protected void configurePipeline(ChannelHandlerContext ctx, String protocol) {
        ChannelPipeline pipeline = ctx.pipeline();
        HTTPConfiguration httpConfiguration = httpLoadBalancer.getHTTPConfiguration();
        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
            pipeline.addLast("HTTP2Handler", HTTPUtils.serverH2Handler(httpConfiguration));
            pipeline.addLast("HTTP2MultiplexHandler", new Http2MultiplexHandler(new MultiplexInitializer(httpLoadBalancer, tlsClient)));
        } else if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_1_1)) {
            pipeline.addLast("HTTPServerCodec", HTTPUtils.newServerCodec(httpConfiguration));
            pipeline.addLast("HTTPServerValidator", new HTTPServerValidator(httpConfiguration));
            pipeline.addLast("HTTPContentCompressor", new HTTPContentCompressor(httpConfiguration));
            pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());
            pipeline.addLast("UpstreamHandler", new UpstreamHandler(httpLoadBalancer, tlsClient));
        } else {
            if (logger.isErrorEnabled()) {
                Throwable throwable = new IllegalArgumentException("Unsupported ALPN Protocol: " + protocol);
                logger.error(throwable);
            }
            ctx.channel().close();
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at ALPN Server Handler", cause);
    }

    private static final class MultiplexInitializer extends ChannelInitializer<Channel> {

        private final HTTPLoadBalancer httpLoadBalancer;
        private final TLSConfiguration tlsClient;

        private MultiplexInitializer(HTTPLoadBalancer httpLoadBalancer, TLSConfiguration tlsClient) {
            this.httpLoadBalancer = httpLoadBalancer;
            this.tlsClient = tlsClient;
        }

        @Override
        protected void initChannel(Channel ch) {
            ChannelPipeline pipeline = ch.pipeline();
            pipeline.addLast("DuplexHTTP2ToHTTPObjectAdapter", new DuplexHTTP2ToHTTPObjectAdapter());
            pipeline.addLast("UpstreamHandler", new UpstreamHandler(httpLoadBalancer, tlsClient, true));
        }
    }
}
