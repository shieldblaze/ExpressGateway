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
package com.shieldblaze.expressgateway.protocol.http.adapter.http2;

import com.shieldblaze.expressgateway.protocol.http.Headers;
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
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p>
 * {@linkplain HTTP2OutboundAdapter} accepts {@linkplain HttpRequest} and {@linkplain HttpContent}
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
 * then {@linkplain HTTP2InboundAdapter} will create {@linkplain FullHttpResponse} else it'll create
 * {@linkplain HttpRequest} and set {@code CONTENT-ENCODING: CHUNKED}.
 * </p>
 */
public final class HTTP2OutboundAdapter extends Http2ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(HTTP2OutboundAdapter.class);

    /**
     * Integer: Connection Stream ID
     * Long: Stream Hash
     */
    private final Map<Integer, Long> streamIds = new ConcurrentSkipListMap<>();

    /**
     * Long: Stream Hash
     * OutboundProperty: {@link OutboundProperty} Instance
     */
    private final Map<Long, OutboundProperty> streamMap = new ConcurrentSkipListMap<>();

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest httpRequest = (HttpRequest) msg;
            Http2FrameCodec.DefaultHttp2FrameStream http2FrameStream = (Http2FrameCodec.DefaultHttp2FrameStream) newStream();
            int streamId = frameCodec.initializeNewStream(ctx, http2FrameStream, promise);
            long streamHash = Long.parseLong(httpRequest.headers().get(Headers.STREAM_HASH));

            // Put the stream ID and Outbound Property into the map.
            OutboundProperty outboundProperty = new OutboundProperty(streamHash, http2FrameStream, httpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));
            streamMap.put(streamHash, outboundProperty);
            streamIds.put(streamId, streamHash);

            if (msg instanceof FullHttpRequest) {
                FullHttpRequest fullHttpRequest = (FullHttpRequest) msg;

                if (fullHttpRequest.content().readableBytes() == 0) {
                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpRequest, false);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, streamHash, http2HeadersFrame, promise);
                } else {
                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpRequest, false);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    writeHeaders(ctx, streamHash, http2HeadersFrame, promise);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                    writeData(ctx, streamHash, dataFrame, ctx.newPromise());
                }
            } else {
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers((HttpRequest) msg, false);
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, streamHash, http2HeadersFrame, promise);
            }
        } else if (msg instanceof HttpContent) {

            if (msg instanceof DefaultHttp2TranslatedLastHttpContent) {
                DefaultHttp2TranslatedLastHttpContent httpContent = (DefaultHttp2TranslatedLastHttpContent) msg;

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                // which will have 'endOfStream' set to 'true.
                if (httpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    writeData(ctx, httpContent.streamHash(), dataFrame, promise);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    writeData(ctx, httpContent.streamHash(), dataFrame, promise);

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpContent.trailingHeaders(), false);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, httpContent.streamHash(), http2HeadersFrame, promise);
                }
            } else if (msg instanceof DefaultHttp2TranslatedHttpContent) {
                DefaultHttp2TranslatedHttpContent httpContent = (DefaultHttp2TranslatedHttpContent) msg;
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, httpContent.streamHash(), dataFrame, promise);
            }
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
            int streamId = headersFrame.stream().id();
            OutboundProperty outboundProperty = getOutboundProperty(streamId);

            HttpVersion httpVersion;
            if (outboundProperty.httpVersion().equals(Headers.Values.HTTP_1_0)) {
                httpVersion = HttpVersion.HTTP_1_0;
            } else if (outboundProperty.httpVersion().equals(Headers.Values.HTTP_1_1) || outboundProperty.httpVersion().equals(Headers.Values.HTTP_2)) {
                httpVersion = HttpVersion.HTTP_1_1;
            } else {
                throw new IllegalArgumentException("Unsupported X-Forwarded-HTTP-Version: " + outboundProperty.httpVersion());
            }

            // If initial read is already performed then this Header frame is part of Last trailing frame.
            if (outboundProperty.initialRead()) {
                LastHttpContent httpContent = new DefaultHttp2TranslatedLastHttpContent(outboundProperty.streamHash());

                HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headersFrame.headers(), httpContent.trailingHeaders(), httpVersion, true, false);
                ctx.fireChannelRead(httpContent);
                removeStreamMapping(streamId);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                HttpResponse httpResponse;
                long streamHash = streamIds.get(streamId);

                // If 'endOfStream' flag is set to 'true' then we will create FullHttpResponse and remove mapping.
                if (headersFrame.isEndStream()) {
                    httpResponse = HttpConversionUtil.toFullHttpResponse(streamId, headersFrame.headers(), Unpooled.EMPTY_BUFFER, httpVersion, false);
                    removeStreamMapping(streamId);
                } else {
                    httpResponse = HttpConversionUtil.toHttpResponse(streamId, headersFrame.headers(), httpVersion, false);
                    httpResponse.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
                }

                httpResponse.headers()
                        .set(Headers.STREAM_HASH, streamHash)
                        .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())
                        .remove(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text())
                        .remove(HttpConversionUtil.ExtensionHeaderNames.PATH.text())
                        .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text())
                        .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_PROMISE_ID.text())
                        .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text());

                outboundProperty.fireInitialRead();
                ctx.fireChannelRead(httpResponse);
            }
        } else if (msg instanceof Http2DataFrame) {
            Http2DataFrame dataFrame = (Http2DataFrame) msg;
            int streamId = dataFrame.stream().id();
            OutboundProperty outboundProperty = getOutboundProperty(streamId);

            Http2TranslatedHttpContent httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(dataFrame.content(), outboundProperty.streamHash());
                removeStreamMapping(streamId);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(dataFrame.content(), outboundProperty.streamHash());
            }

            ctx.fireChannelRead(httpContent);
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, long streamHash, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) throws Exception {
        headersFrame.headers().remove(Headers.STREAM_HASH);
        headersFrame.stream(getOutboundProperty(streamHash).stream());
        super.write(ctx, headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, long streamHash, Http2DataFrame dataFrame, ChannelPromise channelPromise) throws Exception {
        dataFrame.stream(getOutboundProperty(streamHash).stream());
        super.write(ctx, dataFrame, channelPromise);
    }

    /**
     * Get {@linkplain OutboundProperty} using {@code StreamId}
     */
    private OutboundProperty getOutboundProperty(int streamId) {
        OutboundProperty outboundProperty = streamMap.get(streamIds.get(streamId));
        if (outboundProperty == null) {
            throw new IllegalArgumentException("Stream does not exist for StreamID: " + streamId);
        }
        return outboundProperty;
    }

    /**
     * Get {@linkplain OutboundProperty} using {@code StreamHash}
     */
    private OutboundProperty getOutboundProperty(long streamHash) {
        OutboundProperty outboundProperty = streamMap.get(streamHash);
        if (outboundProperty == null) {
            throw new IllegalArgumentException("Stream does not exist for StreamHash: " + streamHash);
        }
        return outboundProperty;
    }

    private void removeStreamMapping(int streamId) {
        streamMap.remove(streamIds.remove(streamId));
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at HTTP2OutboundAdapter", cause);
    }
}
