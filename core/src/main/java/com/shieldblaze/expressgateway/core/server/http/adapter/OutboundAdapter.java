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
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;

import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p>
 * {@linkplain OutboundAdapter} accepts {@linkplain HttpRequest} and {@linkplain HttpContent}
 * and converts them into {@linkplain Http2HeadersFrame} and {@linkplain Http2DataFrame}.
 * </p>
 *
 * <p>
 * For Inbound {@linkplain HttpObject}:
 * <ul>
 * <li>
 *     If {@linkplain HttpResponse} is received then {@linkplain Http2HeadersFrame}
 *     will be sent with {@code endOfStream} set to {@code false}. When {@linkplain LastHttpContent} is received then
 *     {@linkplain Http2DataFrame} is sent with {@code endOfStream} set to {@code true}.
 * </li>
 * <li>
 *     If {@linkplain FullHttpRequest} is received then {@linkplain Http2HeadersFrame}
 *     will be sent with {@code endOfStream} set to {@code false}. A {@linkplain Http2DataFrame}
 *     will be sent with {@linkplain FullHttpRequest#content()} and {@code endOfStream} set to {@code true}.
 * </li>
 * </ul>
 * </p>
 *
 * <p>
 * For Outbound {@linkplain Http2StreamFrame}: If {@linkplain Http2HeadersFrame} has {@code endOfStream} set to {@code true}
 * then {@linkplain InboundAdapter} will create {@linkplain FullHttpResponse} else it'll create
 * {@linkplain HttpRequest} and set {@code CONTENT-ENCODING: CHUNKED}.
 * </p>
 */
public final class OutboundAdapter extends Http2ChannelDuplexHandler {

    /**
     * {@link Integer}: Local Stream ID
     * {@link OutboundProperty}: {@link OutboundProperty} Instance
     */
    private final Map<Integer, OutboundProperty> streamMap = new ConcurrentSkipListMap<>();

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpRequest) {
            Http2FrameCodec.DefaultHttp2FrameStream http2FrameStream = (Http2FrameCodec.DefaultHttp2FrameStream) newStream();
            int streamId = getNextStreamId(ctx, http2FrameStream); // Get the next available stream ID
            http2FrameStream.setId(streamId);

            OutboundProperty outboundProperty = new OutboundProperty(((HttpRequest) msg).headers(), http2FrameStream);
            streamMap.put(streamId, outboundProperty); // Put the stream ID and Outbound Property into the map.

            if (msg instanceof FullHttpRequest) {
                FullHttpRequest fullHttpRequest = (FullHttpRequest) msg;

                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpRequest, false);
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, streamId, http2HeadersFrame, promise);

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                writeData(ctx, streamId, dataFrame, ctx.newPromise());
            } else {
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers((HttpRequest) msg, false);
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, streamId, http2HeadersFrame, promise);
            }
        } else if (msg instanceof HttpContent) {

            Http2DataFrame dataFrame;
            if (msg instanceof DefaultHttp2TranslatedHttpContent) {
                DefaultHttp2TranslatedHttpContent httpContent = (DefaultHttp2TranslatedHttpContent) msg;
                dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, httpContent.getStreamId(), dataFrame, promise);
            } else if (msg instanceof DefaultHttp2TranslatedLastHttpContent) {
                DefaultHttp2TranslatedLastHttpContent httpContent = (DefaultHttp2TranslatedLastHttpContent) msg;

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                // which will have 'endOfStream' set to 'true.
                if (httpContent.trailingHeaders().isEmpty()) {
                    dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    writeData(ctx, httpContent.getStreamId(), dataFrame, promise);
                } else {
                    dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    writeData(ctx, httpContent.getStreamId(), dataFrame, promise);

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpContent.trailingHeaders(), false);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, httpContent.getStreamId(), http2HeadersFrame, promise);
                }
            }
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
            int streamId = headersFrame.stream().id();
            OutboundProperty outboundProperty = getOutboundProperty(streamId);

            if (outboundProperty.isInitialRead()) {
                LastHttpContent httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.EMPTY_BUFFER, outboundProperty.getStreamId(), false);
                HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headersFrame.headers(), httpContent.trailingHeaders(), HttpVersion.HTTP_1_1,
                        true, false);
                ctx.fireChannelRead(httpContent);
                streamMap.remove(streamId);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                HttpResponse httpResponse;

                // If 'endOfStream' flag is set to 'true' then we will create FullHttpResponse.
                if (headersFrame.isEndStream()) {
                    httpResponse = HttpConversionUtil.toFullHttpResponse(streamId, headersFrame.headers(), Unpooled.EMPTY_BUFFER, false);
                    streamMap.remove(streamId);
                } else {
                    httpResponse = HttpConversionUtil.toHttpResponse(streamId, headersFrame.headers(), false);
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_ENCODING, HttpHeaderValues.CHUNKED);
                }

                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), outboundProperty.getScheme());
                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), outboundProperty.getStreamId());
                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), outboundProperty.getDependencyId());
                httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), outboundProperty.getStreamWeight());

                outboundProperty.fireInitialRead();
                ctx.fireChannelRead(httpResponse);
            }
        } else if (msg instanceof Http2DataFrame) {
            Http2DataFrame dataFrame = (Http2DataFrame) msg;
            int streamId = dataFrame.stream().id();
            OutboundProperty outboundProperty = getOutboundProperty(streamId);

            HttpContent httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(dataFrame.content(), outboundProperty.getStreamId(), false);
                streamMap.remove(streamId);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(dataFrame.content(), outboundProperty.getStreamId());
            }

            ctx.fireChannelRead(httpContent);
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, int streamId, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        headersFrame.stream(getOutboundProperty(streamId).getHttp2FrameStream());
        ctx.writeAndFlush(headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, int streamId, Http2DataFrame dataFrame, ChannelPromise channelPromise) {
        dataFrame.stream(getOutboundProperty(streamId).getHttp2FrameStream());
        ctx.writeAndFlush(dataFrame, channelPromise);
    }

    /**
     * Get {@linkplain OutboundProperty} using {@code StreamID}
     */
    private OutboundProperty getOutboundProperty(int streamId) {
        OutboundProperty outboundProperty = streamMap.get(streamId);
        if (outboundProperty == null) {
            throw new IllegalArgumentException("Stream does not exist for StreamID: " + streamId);
        }
        return outboundProperty;
    }

    /**
     * Returns next available Stream ID to use
     *
     * @param ctx {@link ChannelHandlerContext}
     * @return Stream ID
     * @throws IllegalArgumentException If Stream IDs are exhausted on local stream creation
     */
    private int getNextStreamId(ChannelHandlerContext ctx, Http2FrameCodec.DefaultHttp2FrameStream http2FrameStream) {
        Http2FrameCodec http2FrameCodec = ctx.pipeline().get(Http2FrameCodec.class);
        int streamId = http2FrameCodec.connection().local().incrementAndGetNextStreamId();
        if (streamId < 0) {
            throw new IllegalArgumentException("Stream IDs exhausted on local stream creation");
        }
        http2FrameCodec.frameStreamToInitializeMap.put(streamId, http2FrameStream);
        return streamId;
    }
}
