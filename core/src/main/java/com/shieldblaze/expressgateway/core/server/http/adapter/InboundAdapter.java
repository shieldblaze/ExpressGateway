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
package com.shieldblaze.expressgateway.core.server.http.adapter;

import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;

import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p>
 * {@linkplain InboundAdapter} is responsible for handling {@link Http2HeadersFrame} and
 * {@link Http2DataFrame} and converts them into {@link HttpRequest} and {@link HttpContent}.
 * </p>
 */
public final class InboundAdapter extends ChannelDuplexHandler {

    private final Map<Integer, Http2FrameStream> streamMap = new ConcurrentSkipListMap<>();

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
            int streamId = headersFrame.stream().id();

            if (streamMap.containsKey(streamId)) {
                DefaultHttp2TranslatedLastHttpContent httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.EMPTY_BUFFER, streamId, false);
                HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headersFrame.headers(), httpContent.trailingHeaders(), HttpVersion.HTTP_1_1, true, true);
                ctx.fireChannelRead(httpContent);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                HttpRequest httpRequest = HttpConversionUtil.toHttpRequest(streamId, headersFrame.headers(), false);
                ctx.fireChannelRead(httpRequest);
                streamMap.put(streamId, headersFrame.stream());
            }

        } else if (msg instanceof Http2DataFrame) {
            Http2DataFrame dataFrame = (Http2DataFrame) msg;
            int streamId = dataFrame.stream().id();

            HttpContent httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(dataFrame.content(), streamId, false);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(dataFrame.content(), streamId);
            }
            ctx.fireChannelRead(httpContent);
        }
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpResponse) {
            HttpResponse httpResponse = (HttpResponse) msg;
            int streamId = httpResponse.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
            Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpResponse, false);

            Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false, 0);
            headersFrame.stream(streamMap.get(streamId));
            ctx.writeAndFlush(headersFrame, promise);
        } else if (msg instanceof HttpContent) {
            HttpContent httpContent = (HttpContent) msg;
            Http2DataFrame dataFrame;

            if (msg instanceof DefaultHttp2TranslatedHttpContent) {
                int streamId = ((DefaultHttp2TranslatedHttpContent) msg).getStreamId();
                dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                dataFrame.stream(streamMap.get(streamId));
                ctx.writeAndFlush(dataFrame, promise);
            } else if (msg instanceof DefaultHttp2TranslatedLastHttpContent) {
                DefaultHttp2TranslatedLastHttpContent lastHttpContent = (DefaultHttp2TranslatedLastHttpContent) msg;
                int streamId = lastHttpContent.getStreamId();
                dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), true);
                dataFrame.stream(streamMap.get(streamId));

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                // which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), true);
                    writeData(ctx, lastHttpContent.getStreamId(), dataFrame, promise); // Write and flush the HTTP/2 Data Frame
                } else {
                    dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), false);
                    writeData(ctx, lastHttpContent.getStreamId(), dataFrame, promise); // Write and flush the HTTP/2 Data Frame

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), false);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true, 0);
                    http2HeadersFrame.stream(streamMap.get(streamId));
                    ctx.writeAndFlush(http2HeadersFrame, promise); // Write and flush the HTTP/2 Header Frame
                }
            }
        }
    }

    private void writeData(ChannelHandlerContext ctx, int streamId, Http2DataFrame dataFrame, ChannelPromise channelPromise) {
        dataFrame.stream(streamMap.get(streamId));
        ctx.writeAndFlush(dataFrame, channelPromise);
    }
}
