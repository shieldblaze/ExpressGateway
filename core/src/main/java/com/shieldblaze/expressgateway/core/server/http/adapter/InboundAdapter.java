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
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpObject;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
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
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p>
 * {@linkplain InboundAdapter} accepts {@linkplain Http2HeadersFrame} and {@linkplain Http2DataFrame}
 * and converts them into {@linkplain HttpRequest} and {@linkplain HttpContent}.
 * </p>
 *
 * <p>
 * For Inbound {@linkplain Http2StreamFrame}: If {@linkplain Http2HeadersFrame} has {@code endOfStream} set to {@code true}
 * then {@linkplain InboundAdapter} will create {@linkplain FullHttpRequest} else it'll create
 * {@linkplain HttpRequest} and set {@code CONTENT-ENCODING: CHUNKED}.
 * </p>
 *
 * <p>
 * For Outbound {@linkplain HttpObject}:
 * <ul>
 * <li>
 *     If {@linkplain HttpResponse} is received then {@linkplain Http2HeadersFrame}
 *     will be sent with {@code endOfStream} set to {@code false}. When {@linkplain LastHttpContent} is received then
 *     {@linkplain Http2DataFrame} is sent with {@code endOfStream} set to {@code true}.
 * </li>
 * <li>
 *     If {@linkplain FullHttpResponse} is received then {@linkplain Http2HeadersFrame}
 *     will be sent with {@code endOfStream} set to {@code false}. A {@linkplain Http2DataFrame}
 *     will be sent with {@linkplain FullHttpResponse#content()} and {@code endOfStream} set to {@code true}.
 * </li>
 * </ul>
 * </p>
 */
public final class InboundAdapter extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(InboundAdapter.class);

    private final Map<Integer, Http2FrameStream> streamMap = new ConcurrentSkipListMap<>();

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
            int streamId = headersFrame.stream().id();

            // If StreamMap already contains the streamId then this HeadersFrame is part of Trailing Headers.
            if (streamMap.containsKey(streamId)) {
                LastHttpContent httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.EMPTY_BUFFER, streamId, false);
                HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headersFrame.headers(), httpContent.trailingHeaders(), HttpVersion.HTTP_1_1, true, true);
                ctx.fireChannelRead(httpContent);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                HttpRequest httpRequest;
                // If 'endOfStream' is set to 'true' then we'll create 'FullHttpRequest' else 'HttpRequest'.
                if (headersFrame.isEndStream()) {
                    httpRequest = HttpConversionUtil.toFullHttpRequest(streamId, headersFrame.headers(), Unpooled.EMPTY_BUFFER, false);
                } else {
                    httpRequest = HttpConversionUtil.toHttpRequest(streamId, headersFrame.headers(), false);
                    httpRequest.headers().set(HttpHeaderNames.CONTENT_ENCODING, HttpHeaderValues.CHUNKED);
                }
                streamMap.put(streamId, headersFrame.stream());
                ctx.fireChannelRead(httpRequest);
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
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpResponse) {
            if (msg instanceof FullHttpResponse) {
                FullHttpResponse fullHttpResponse = (FullHttpResponse) msg;
                int streamId = fullHttpResponse.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpResponse, false);

                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, streamId, headersFrame, promise);

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpResponse.content(), true);
                writeData(ctx, streamId, dataFrame, ctx.newPromise());

                streamMap.remove(streamId); // We're done with this Stream
            } else {
                HttpResponse httpResponse = (HttpResponse) msg;
                int streamId = httpResponse.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpResponse, false);

                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, streamId, headersFrame, promise);
            }
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
                    writeData(ctx, lastHttpContent.getStreamId(), dataFrame, promise);
                } else {
                    dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), false);
                    writeData(ctx, lastHttpContent.getStreamId(), dataFrame, promise);

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), false);
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, streamId, headersFrame, promise);
                }

                streamMap.remove(streamId); // We're done with this Stream
            }
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, int streamId, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        headersFrame.stream(streamMap.get(streamId));
        ctx.writeAndFlush(headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, int streamId, Http2DataFrame dataFrame, ChannelPromise channelPromise) {
        dataFrame.stream(streamMap.get(streamId));
        ctx.writeAndFlush(dataFrame, channelPromise);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at InboundAdapter", cause);
    }
}
