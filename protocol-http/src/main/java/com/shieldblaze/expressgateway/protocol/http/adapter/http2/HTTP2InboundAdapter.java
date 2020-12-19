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

import com.shieldblaze.expressgateway.protocol.http.HTTPConversionUtil;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTP2ContentCompressor;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPCompressionUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.CustomFullHttpResponse;
import io.netty.handler.codec.http.CustomHttpContent;
import io.netty.handler.codec.http.CustomHttpResponse;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpObject;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.SplittableRandom;
import java.util.concurrent.ConcurrentHashMap;

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

    /**
     * Logger
     */
    private static final Logger logger = LogManager.getLogger(HTTP2InboundAdapter.class);

    /**
     * Fast Random Number Generator
     */
    private static final SplittableRandom RANDOM = new SplittableRandom();

    /**
     * <p> {@link Long}: HTTP Request ID </p>
     * <p> {@link Integer}: HTTP/2 Stream ID  </p>
     */
    private final Map<Long, Integer> requestIdToStreamIdMap = new ConcurrentHashMap<>();

    /**
     * <p> {@link Integer}: HTTP/2 Stream ID </p>
     * <p> {@link InboundProperty}: {@linkplain InboundProperty} associated with the HTTP Request.  </p>
     */
    private final Map<Integer, InboundProperty> streamIdMap = new ConcurrentHashMap<>();

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

        /*
         * First condition:
         *      If StreamID Map already contains the streamId then this Header Frame is part of Trailing Headers.
         *      We'll
         *
         */
        if (streamIdMap.containsKey(streamId)) {
            InboundProperty property = getStream(streamId);

            LastHttpContent httpContent = new CustomLastHttpContent(Unpooled.EMPTY_BUFFER, HttpFrame.Protocol.HTTP_1_1, property.httpFrame().id());
            HTTPConversionUtil.addHttp2ToHttpHeaders(headersFrame.headers(), httpContent.trailingHeaders(), true, true);
            ctx.fireChannelRead(httpContent);

            // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
            if (!headersFrame.isEndStream()) {
                ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.PROTOCOL_ERROR).setExtraStreamIds(streamId), ctx.voidPromise());
                removeStream(property);
            }
        } else {
            long id = RANDOM.nextLong();

            HttpRequest httpRequest;
            // If 'endOfStream' is set to 'true' then we'll create 'FullHttpRequest' else 'HttpRequest'.
            if (headersFrame.isEndStream()) {
                httpRequest = HTTPConversionUtil.toFullHttpRequest(id, headersFrame.headers(), Unpooled.EMPTY_BUFFER);
            } else {
                httpRequest = HTTPConversionUtil.toHttpRequest(id, headersFrame.headers());
                httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
            }

            String acceptEncoding;
            if (headersFrame.headers().contains(HttpHeaderNames.ACCEPT_ENCODING)) {
                acceptEncoding = headersFrame.headers().get(HttpHeaderNames.ACCEPT_ENCODING).toString();
            } else {
                acceptEncoding = null;
            }

            HttpFrame httpFrame = (HttpFrame) httpRequest;
            InboundProperty property = new InboundProperty(httpFrame, headersFrame.stream(), acceptEncoding);
            addStream(property);

            ctx.fireChannelRead(httpRequest);
        }
    }

    private void onHttp2DataRead(ChannelHandlerContext ctx, Http2DataFrame dataFrame) {
        long id = getStream(dataFrame.stream().id()).httpFrame().id();

        HttpContent httpContent;
        if (dataFrame.isEndStream()) {
            httpContent = new DefaultHttp2TranslatedLastHttpContent(dataFrame.content(), id, false);
        } else {
            httpContent = new DefaultHttp2TranslatedHttpContent(dataFrame.content(), id);
        }
        ctx.fireChannelRead(httpContent);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof HttpResponse) {
            if (msg instanceof FullHttpResponse) {
                CustomFullHttpResponse httpResponse = (CustomFullHttpResponse) msg;
                InboundProperty inboundProperty = getStream(httpResponse.id());
                Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(httpResponse);

                applyCompression(http2Headers, inboundProperty);

                // If 'readableBytes' is 0 then there is no Data frame to write. We'll mark Header frame as 'endOfStream'.
                if (httpResponse.content().readableBytes() == 0) {
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, inboundProperty, headersFrame, promise);
                } else {
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    writeHeaders(ctx, inboundProperty, headersFrame, promise);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpResponse.content(), true);
                    writeData(ctx, inboundProperty, dataFrame, ctx.voidPromise(), true);
                }

                // We're done with this Stream
                removeStream(inboundProperty);
            } else {
                CustomHttpResponse httpResponse = (CustomHttpResponse) msg;
                InboundProperty inboundProperty = getStream(httpResponse.id());
                Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(httpResponse);

                applyCompression(http2Headers, inboundProperty);

                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, inboundProperty, headersFrame, promise);
            }
        } else if (msg instanceof HttpContent) {
            if (msg instanceof CustomLastHttpContent) {
                CustomLastHttpContent lastHttpContent = (CustomLastHttpContent) msg;
                InboundProperty property = getStream(lastHttpContent.id());

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                //   which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), true);
                    writeData(ctx, property, dataFrame, promise, false);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), false);
                    writeData(ctx, property, dataFrame, promise, false);

                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders());
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, property, headersFrame, ctx.voidPromise());
                }

                // We're done with this Stream
                removeStream(property);
            } else if (msg instanceof CustomHttpContent) {
                CustomHttpContent httpContent = (CustomHttpContent) msg;
                InboundProperty property = getStream(httpContent.id());

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, property, dataFrame, promise, false);
            }
        }
    }

    /**
     * <p> Determine whether compression can be applied or not. </p>
     *
     * <p> If {@link Http2Headers} does not contain 'CONTENT-ENCODING' and
     * {@link InboundProperty#acceptEncoding()} is not 'null' then
     * we'll call {@link HTTPCompressionUtil#targetEncoding(Http2Headers, String)}
     * to determine whether the content is compressible or not.
     * If content is compressible then we'll add 'CONTENT-ENCODING' headers so it
     * can be compressed by {@link HTTP2ContentCompressor}. </p>
     */
    private void applyCompression(Http2Headers headers, InboundProperty inboundProperty) {
        if (!headers.contains(HttpHeaderNames.CONTENT_ENCODING) && inboundProperty.acceptEncoding() != null) {
            String targetEncoding = HTTPCompressionUtil.targetEncoding(headers, inboundProperty.acceptEncoding());
            if (targetEncoding != null) {
                headers.set(HttpHeaderNames.CONTENT_ENCODING, targetEncoding);
            }
        }
    }

    /**
     * Write {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, InboundProperty inboundProperty, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        headersFrame.stream(inboundProperty.stream());
        ctx.write(headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, InboundProperty inboundProperty, Http2DataFrame dataFrame, ChannelPromise channelPromise, boolean flush) {
        dataFrame.stream(inboundProperty.stream());
        if (flush) {
            ctx.writeAndFlush(dataFrame, channelPromise);
        } else {
            ctx.write(dataFrame, channelPromise);
        }
    }

    private void addStream(InboundProperty inboundProperty) {
        requestIdToStreamIdMap.put(inboundProperty.httpFrame().id(), inboundProperty.stream().id());
        streamIdMap.put(inboundProperty.stream().id(), inboundProperty);
    }

    private void removeStream(InboundProperty inboundProperty) {
        requestIdToStreamIdMap.put(inboundProperty.httpFrame().id(), inboundProperty.stream().id());
        streamIdMap.put(inboundProperty.stream().id(), inboundProperty);
    }

    private InboundProperty getStream(long id) {
        return getStream(requestIdToStreamIdMap.get(id));
    }

    private InboundProperty getStream(int streamId) {
        return streamIdMap.get(streamId);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at HTTP2InboundAdapter", cause);
    }
}
