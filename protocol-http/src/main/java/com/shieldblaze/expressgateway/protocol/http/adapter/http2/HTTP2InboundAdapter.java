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
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPCompressionUtil;
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
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.Random;
import java.util.concurrent.ConcurrentSkipListMap;

/**
 * <p>
 * {@linkplain HTTP2InboundAdapter} accepts {@linkplain Http2HeadersFrame} and {@linkplain Http2DataFrame}
 * and converts them into {@linkplain HttpRequest} and {@linkplain HttpContent}.
 * </p>
 *
 * <p>
 * For Inbound {@linkplain Http2StreamFrame}: If {@linkplain Http2HeadersFrame} has {@code endOfStream} set to {@code true}
 * then {@linkplain HTTP2InboundAdapter} will create {@linkplain FullHttpRequest} else it'll create
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
public final class HTTP2InboundAdapter extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(HTTP2InboundAdapter.class);

    /**
     * Integer: Connection Stream ID
     * Long: Stream Hash
     */
    private final Map<Integer, Long> streamIds = new ConcurrentSkipListMap<>();

    /**
     * Long: Stream Hash
     * InboundProperty: {@link InboundProperty} Instance
     */
    private final Map<Long, InboundProperty> streamMap = new ConcurrentSkipListMap<>();

    private Random random;

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        if (ctx.channel().remoteAddress() != null) {
            random = new Random(System.nanoTime() + ctx.channel().remoteAddress().hashCode());
        } else {
            // This happens when we're using EmbeddedChannel during testing.
            random = new Random();
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            onHttp2HeadersRead(ctx, (Http2HeadersFrame) msg);
        } else if (msg instanceof Http2DataFrame) {
            onHttp2DataRead(ctx, (Http2DataFrame) msg);
        } else {
            // Unsupported message type
        }
    }

    private void onHttp2HeadersRead(ChannelHandlerContext ctx, Http2HeadersFrame headersFrame) throws Http2Exception {
        int streamId = headersFrame.stream().id();

        // If StreamMap already contains the streamId then this HeadersFrame is part of Trailing Headers.
        if (streamIds.containsKey(streamId)) {
            LastHttpContent httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.EMPTY_BUFFER, streamId, false);
            HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headersFrame.headers(), httpContent.trailingHeaders(), HttpVersion.HTTP_1_1, true, true);
            ctx.fireChannelRead(httpContent);

            // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
            if (!headersFrame.isEndStream()) {
                ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                removeStreamMapping(streamId);
            }
        } else {
            HttpRequest httpRequest;
            // If 'endOfStream' is set to 'true' then we'll create 'FullHttpRequest' else 'HttpRequest'.
            if (headersFrame.isEndStream()) {
                httpRequest = HttpConversionUtil.toFullHttpRequest(streamId, headersFrame.headers(), ctx.alloc(), false);
            } else {
                httpRequest = HttpConversionUtil.toHttpRequest(streamId, headersFrame.headers(), false);
                httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
            }

            String acceptEncoding;
            if (headersFrame.headers().contains(HttpHeaderNames.ACCEPT_ENCODING)) {
                acceptEncoding = headersFrame.headers().get(HttpHeaderNames.ACCEPT_ENCODING).toString();
            } else {
                acceptEncoding = null;
            }

            InboundProperty inboundProperty = new InboundProperty(random.nextLong(), headersFrame.stream(), acceptEncoding);
            streamMap.put(inboundProperty.streamHash(), inboundProperty);
            streamIds.put(streamId, inboundProperty.streamHash());

            httpRequest.headers()
                    .set(Headers.STREAM_HASH, inboundProperty.streamHash())
                    .set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_2)
                    .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text())
                    .remove(HttpConversionUtil.ExtensionHeaderNames.PATH.text())
                    .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text())
                    .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_PROMISE_ID.text())
                    .remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text());

            ctx.fireChannelRead(httpRequest);
        }
    }

    private void onHttp2DataRead(ChannelHandlerContext ctx, Http2DataFrame dataFrame) {
        int streamId = dataFrame.stream().id();

        HttpContent httpContent;
        if (dataFrame.isEndStream()) {
            httpContent = new DefaultHttp2TranslatedLastHttpContent(dataFrame.content(), streamId, false);
        } else {
            httpContent = new DefaultHttp2TranslatedHttpContent(dataFrame.content(), streamId);
        }
        ctx.fireChannelRead(httpContent);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpResponse) {
            if (msg instanceof FullHttpResponse) {
                FullHttpResponse fullHttpResponse = (FullHttpResponse) msg;
                InboundProperty inboundProperty = streamMap.get(Long.parseLong(fullHttpResponse.headers().get(Headers.STREAM_HASH)));
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpResponse, false);
                http2Headers.remove(Headers.STREAM_HASH);

                applyCompression(http2Headers, inboundProperty);

                // If 'readableBytes' is 0 then there is no Data frame to write. We'll mark Header frame as 'endOfStream'.
                if (fullHttpResponse.content().readableBytes() == 0) {
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, inboundProperty, headersFrame, promise);
                } else {
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    writeHeaders(ctx, inboundProperty, headersFrame, promise);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpResponse.content(), true);
                    writeData(ctx, inboundProperty, dataFrame, ctx.newPromise());
                }

                // We're done with this Stream
                removeStreamMapping(inboundProperty.streamHash());
            } else {
                HttpResponse httpResponse = (HttpResponse) msg;
                InboundProperty inboundProperty = streamMap.get(Long.parseLong(httpResponse.headers().get(Headers.STREAM_HASH)));
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpResponse, false);

                applyCompression(http2Headers, inboundProperty);

                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, inboundProperty, headersFrame, promise);
            }
        } else if (msg instanceof Http2TranslatedHttpContent) {
            Http2TranslatedHttpContent httpContent = (Http2TranslatedHttpContent) msg;

            if (msg instanceof DefaultHttp2TranslatedLastHttpContent) {
                DefaultHttp2TranslatedLastHttpContent lastHttpContent = (DefaultHttp2TranslatedLastHttpContent) msg;
                long streamHash = lastHttpContent.streamHash();
                Http2DataFrame dataFrame;

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                // which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), true);
                    writeData(ctx, streamHash, dataFrame, promise);
                } else {
                    dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), false);
                    writeData(ctx, streamHash, dataFrame, promise);

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), false);
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, streamHash, headersFrame, ctx.newPromise());
                }

                // We're done with this Stream
                removeStreamMapping(streamHash);
            } else if (msg instanceof DefaultHttp2TranslatedHttpContent) {
                long streamHash = ((DefaultHttp2TranslatedHttpContent) msg).streamHash();

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, streamHash, dataFrame, promise);
            }
        }
    }

    private void applyCompression(Http2Headers headers, InboundProperty inboundProperty) {
        if (!headers.contains(HttpHeaderNames.CONTENT_ENCODING) && inboundProperty.acceptEncoding() != null) {
            String targetEncoding = HTTPCompressionUtil.targetEncoding(headers, inboundProperty.acceptEncoding());
            if (targetEncoding != null) {
                headers.set(HttpHeaderNames.CONTENT_ENCODING, targetEncoding);
            }
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, long streamHash, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        writeHeaders(ctx, streamMap.get(streamHash), headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, InboundProperty inboundProperty, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        headersFrame.stream(inboundProperty.stream());
        ctx.write(headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, long streamHash, Http2DataFrame dataFrame, ChannelPromise channelPromise) {
        writeData(ctx, streamMap.get(streamHash), dataFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, InboundProperty inboundProperty, Http2DataFrame dataFrame, ChannelPromise channelPromise) {
        dataFrame.stream(inboundProperty.stream());
        ctx.write(dataFrame, channelPromise);
    }

    protected Long streamHash(int streamId) {
        return streamIds.get(streamId);
    }

    protected InboundProperty streamProperty(long streamHash) {
        return streamMap.get(streamHash);
    }

    protected void removeStreamMapping(int streamId) {
        streamMap.remove(streamIds.remove(streamId));
    }

    protected void removeStreamMapping(long streamHash) {
        InboundProperty inboundProperty = streamMap.remove(streamHash);
        streamIds.remove(inboundProperty.stream().id());
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at HTTP2InboundAdapter", cause);
    }
}
