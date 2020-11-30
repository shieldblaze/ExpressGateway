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
package com.shieldblaze.expressgateway.protocol.http.adapter.http1;

import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2OutboundAdapter;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;
import net.openhft.hashing.LongHashFunction;

import java.util.Random;
import java.util.concurrent.atomic.AtomicLong;

/**
 * {@linkplain HTTPInboundAdapter} handles incoming HTTP/1.x requests
 * and add headers so they can be processed by {@linkplain HTTPOutboundAdapter}
 * or {@linkplain HTTP2OutboundAdapter}.
 */
public final class HTTPInboundAdapter extends ChannelDuplexHandler {

    private final AtomicLong streamHash = new AtomicLong();
    private Random random;

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        if (ctx.channel().remoteAddress() != null) {
            random = new Random(System.nanoTime() + ctx.channel().remoteAddress().hashCode());
        } else {
            random = new Random();
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            streamHash.set(LongHashFunction.xx().hashLong(random.nextLong()));

            request.headers().set(Headers.STREAM_HASH, streamHash)
                    .set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");

            if (request.protocolVersion() == HttpVersion.HTTP_1_0) {
                request.headers().set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_1_0);
            } else {
                request.headers().set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_1_1);
            }

            ctx.fireChannelRead(request);
        } else if (msg instanceof HttpContent) {

            HttpContent httpContent;
            if (msg instanceof LastHttpContent) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(((LastHttpContent) msg).content(), streamHash.get());
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(((HttpContent) msg).content(), streamHash.get());
            }

            ctx.fireChannelRead(httpContent);
        }
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) msg;
            response.headers().remove(Headers.STREAM_HASH);
        }
        ctx.write(msg, promise);
    }
}
