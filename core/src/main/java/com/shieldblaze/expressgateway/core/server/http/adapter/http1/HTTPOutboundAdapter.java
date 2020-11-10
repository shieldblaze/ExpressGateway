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
package com.shieldblaze.expressgateway.core.server.http.adapter.http1;

import com.shieldblaze.expressgateway.core.server.http.Headers;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;

public final class HTTPOutboundAdapter extends ChannelDuplexHandler {

    private long streamHash = 0L;

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;
            streamHash = Long.parseLong(request.headers().get(Headers.STREAM_HASH));

            request.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
            request.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text());
            request.headers().remove(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text());
            request.headers().remove(HttpConversionUtil.ExtensionHeaderNames.PATH.text());
            request.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text());
        }
        ctx.write(msg, promise);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) msg;
            response.headers().set(Headers.STREAM_HASH, streamHash);
        } else if (msg instanceof HttpContent) {

            HttpContent httpContent;
            if (msg instanceof LastHttpContent) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(((LastHttpContent) msg).content(), streamHash);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(((HttpContent) msg).content(), streamHash);
            }

            ctx.fireChannelRead(httpContent);
            return;
        }

        ctx.fireChannelRead(msg);
    }
}
