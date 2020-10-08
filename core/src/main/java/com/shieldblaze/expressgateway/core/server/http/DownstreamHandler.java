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

import com.shieldblaze.expressgateway.core.server.http.compression.HTTPContentCompressor;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponse;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final Channel upstream;
    private final InetSocketAddress upstreamAddress;
    private final Backend backend;
    private final Map<Integer, String> acceptEncodingMap;
    private final boolean isUpstreamHTTP2;

    DownstreamHandler(Channel upstream, Backend backend, Map<Integer, String> acceptEncodingMap, boolean isUpstreamHTTP2) {
        this.upstream = upstream;
        this.upstreamAddress = (InetSocketAddress) upstream.remoteAddress();
        this.backend = backend;
        this.acceptEncodingMap = acceptEncodingMap;
        this.isUpstreamHTTP2 = isUpstreamHTTP2;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) msg;
            HTTPUtils.setGenericHeaders(response.headers());

            if (isUpstreamHTTP2) {
                if (response.headers().contains(HttpHeaderNames.CONTENT_ENCODING)) {
                    String acceptEncoding = acceptEncodingMap.get(response.headers().getInt("x-http2-stream-id"));
                    if (acceptEncoding != null) {
                        String targetContentEncoding = HTTPContentCompressor.getTargetEncoding(response, acceptEncoding);
                        if (targetContentEncoding != null) {
                            response.headers().set(HttpHeaderNames.CONTENT_ENCODING, targetContentEncoding);
                        }
                    }
                }
                acceptEncodingMap.remove(response.headers().getInt("x-http2-stream-id"));
            }
        }
        upstream.writeAndFlush(msg);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    upstreamAddress.getAddress().getHostAddress() + ":" + upstreamAddress.getPort(),
                    backend.getSocketAddress().getAddress().getHostAddress() + ":" + backend.getSocketAddress().getPort());
        }

        if (ctx.channel().isActive()) {
            ctx.close();
        }

        if (upstream.isActive()) {
            upstream.close();
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
