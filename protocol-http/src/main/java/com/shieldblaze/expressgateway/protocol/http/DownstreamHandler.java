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
package com.shieldblaze.expressgateway.protocol.http;

import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final HTTPConnection httpConnection;
    private Channel channel;
    private boolean doCloseAtLast;

    DownstreamHandler(HTTPConnection httpConnection, Channel channel) {
        this.httpConnection = httpConnection;
        this.channel = channel;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof Http2HeadersFrame || msg instanceof Http2DataFrame) {
            // No need of any modification
        } else if (msg instanceof HttpResponse) {

            if (msg instanceof FullHttpResponse) {
                if (((FullHttpResponse) msg).protocolVersion() == HttpVersion.HTTP_1_0) {
                    ((FullHttpResponse) msg).headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
                    channel.writeAndFlush(msg).addListener(ChannelFutureListener.CLOSE);
                    return;
                }
            } else {
                // If HTTP Version is 1.0 then set header 'Connection:Close' and mark for CloseAtLast.
                if (((HttpResponse) msg).protocolVersion() == HttpVersion.HTTP_1_0) {
                    doCloseAtLast = true;
                    ((HttpResponse) msg).headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
                }
            }

        } else if (msg instanceof LastHttpContent) {

            // If Connection is not over HTTP/2 then release it back to pool.
            if (!httpConnection.isHTTP2()) {
                httpConnection.release();
            }

            if (doCloseAtLast) {
                doCloseAtLast = false;
                channel.writeAndFlush(msg).addListener(ChannelFutureListener.CLOSE);
                return;
            }
        }

        channel.writeAndFlush(msg);
    }

    void setChannel(Channel channel) {
        this.channel = channel;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
