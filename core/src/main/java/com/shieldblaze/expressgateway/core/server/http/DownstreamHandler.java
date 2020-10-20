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

import com.shieldblaze.expressgateway.core.utils.ChannelUtils;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final UpstreamHandler upstreamHandler;
    boolean isActive;

    DownstreamHandler(UpstreamHandler upstreamHandler) {
        this.upstreamHandler = upstreamHandler;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) msg;
            HTTPUtils.setGenericHeaders(response.headers());

            /*
             * If `isUpstreamHTTP2` is `true` then  we'll lookup at map using Stream-ID and check if we have entry for "ACCEPT-ENCODING" values.
             */
            if (upstreamHandler.isHTTP2) {
                String acceptEncoding = upstreamHandler.acceptEncodingMap.get(response.headers().getInt("x-http2-stream-id"));
                if (acceptEncoding != null) {
                    String targetContentEncoding = HTTPContentCompressor.getTargetEncoding(response, acceptEncoding);
                    if (targetContentEncoding != null) {
                        response.headers().set(HttpHeaderNames.CONTENT_ENCODING, targetContentEncoding);
                    }
                    upstreamHandler.acceptEncodingMap.remove(response.headers().getInt("x-http2-stream-id"));
                }
            }
        }
        if (msg instanceof LastHttpContent) {
            isActive = false;
        }
        upstreamHandler.upstreamChannel.writeAndFlush(msg);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    upstreamHandler.upstreamAddress.getAddress().getHostAddress() + ":" + upstreamHandler.upstreamAddress.getPort(),
                    upstreamHandler.downstreamAddress.getAddress().getHostAddress() + ":" + upstreamHandler.downstreamAddress.getPort());
        }

        ChannelUtils.closeChannels(upstreamHandler.upstreamChannel, upstreamHandler.downstreamChannel);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
