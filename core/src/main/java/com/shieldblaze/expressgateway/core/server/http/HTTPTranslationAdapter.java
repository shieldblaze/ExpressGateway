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

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.HttpConversionUtil;

/**
 * <p>
 * {@link HTTPTranslationAdapter} is Full-Duplex Adapter for handling Translation of:
 *     <ul>
 *         <li>
 *             {@link UpstreamHandler} HTTP/2 to {@link DownstreamHandler} HTTP/1.1
 *         </li>
 *         <li>
 *             {@link UpstreamHandler} HTTP/1.1 to {@link DownstreamHandler} HTTP/2
 *         </li>
 *     </ul>
 * </p>
 */
final class HTTPTranslationAdapter extends ChannelDuplexHandler {

    private final boolean isUpstreamHTTP2;
    private int streamId;
    private short weight;
    private int dependencyId;
    private int promiseId;

    /**
     * Create a new {@link HTTPTranslationAdapter} Instance
     *
     * @param isUpstreamHTTP2 Set to {@code true} if {@link UpstreamHandler} is using HTTP/2
     *                        else set to {@code false}.
     */
    HTTPTranslationAdapter(boolean isUpstreamHTTP2) {
        this.isUpstreamHTTP2 = isUpstreamHTTP2;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpResponse) {
            HttpResponse httpResponse = (HttpResponse) msg;

            if (isUpstreamHTTP2) {
                if (streamId != -1) {
                    httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), streamId);
                }
                if (weight != -1) {
                    httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), weight);
                }
                if (dependencyId != -1) {
                    httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), dependencyId);
                }
                if (promiseId != -1) {
                    httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_PROMISE_ID.text(), promiseId);
                }
            } else {
                httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
                httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text());
                httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text());
                httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_PROMISE_ID.text());
                httpResponse.headers().remove(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text());
            }
        }
        super.channelRead(ctx, msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest httpRequest = (HttpRequest) msg;

            if (isUpstreamHTTP2) {
                streamId = httpRequest.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), -1);
                weight = httpRequest.headers().getShort(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), (short) -1);
                dependencyId = httpRequest.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), -1);
                promiseId = httpRequest.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_PROMISE_ID.text(), -1);

                if (streamId == -1) {
                    throw new Http2Exception(Http2Error.PROTOCOL_ERROR, "StreamID not found");
                }
            } else {
                httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
            }
        }
        super.write(ctx, msg, promise);
    }
}
