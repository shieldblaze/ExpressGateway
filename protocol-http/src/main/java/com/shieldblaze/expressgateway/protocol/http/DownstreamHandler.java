/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.atomic.AtomicBoolean;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final AtomicBoolean doCloseAtLast = new AtomicBoolean(false);
    private final HTTPConnection httpConnection;
    private Channel channel;

    DownstreamHandler(HTTPConnection httpConnection, Channel channel) {
        this.httpConnection = httpConnection;
        this.channel = channel;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (httpConnection.isHTTP2()) {
            handleHTTP2(msg);
        } else {
            handleHTTP1(msg);
        }
    }

    private void handleHTTP2(Object msg) {
        // Just write data into channel.
        channel.writeAndFlush(msg, channel.voidPromise());
    }

    private void handleHTTP1(Object msg) {
        if (msg instanceof HttpResponse) {
            HttpResponse httpResponse = (HttpResponse) msg;

            /*
             * If Protocol version is HTTP/1.0 then we'll add 'CONNECTION:CLOSE' header.
             * And if the Response is FullHttpResponse then we'll write the message and release and close the connection.
             * If Response is not FullHttpResponse then we'll set `doCloseAtLast` to true.
             *
             *
             * If Protocol version is HTTP/1.1 and the response is FullHttpResponse then we'll add listener to release connection
             * when message is successfully written.
             */
            if (httpResponse.protocolVersion() == HttpVersion.HTTP_1_0) {
                httpResponse.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);

                if (httpResponse instanceof FullHttpResponse) {
                    channel.writeAndFlush(httpResponse)
                            .addListener(future -> httpConnection.release())
                            .addListener(ChannelFutureListener.CLOSE);
                } else {
                    doCloseAtLast.set(true);
                }
            } else {
                if (httpResponse instanceof FullHttpResponse) {
                    channel.writeAndFlush(httpResponse)
                            .addListener(future -> httpConnection.release());
                    return;
                }
            }
        } else if (msg instanceof LastHttpContent) {
            ChannelFuture channelFuture = channel.writeAndFlush(msg)
                    .addListener(future -> httpConnection.release());

            if (doCloseAtLast.compareAndSet(true, false)) {
                channelFuture.addListener(ChannelFutureListener.CLOSE);
            }

            return;
        }

        // HTTPContent writes will be executed here.
        channel.writeAndFlush(msg, channel.voidPromise());
    }

    void channel(Channel channel) {
        this.channel = channel;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);
    }
}
