/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.CustomHttpContent;
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
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.lang.reflect.Field;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

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
 * {@linkplain HttpRequest} and set {@code TRANSFER_ENCODING: CHUNKED}.
 * </p>
 */
public final class HTTP2OutboundAdapter extends Http2ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(HTTP2OutboundAdapter.class);

    private Http2FrameCodec FRAME_CODEC_INSTANCE;
    private Method FRAME_CODEC_METHOD;

    /**
     * <p> Integer: HTTP/2 Stream ID </p>
     * <p> Long: Request ID </p>
     */
    private final Map<Integer, Long> requestIdToStreamIdMap = new ConcurrentHashMap<>();

    /**
     * <p> Long: Request ID </p>
     * <p> OutboundProperty: {@link OutboundProperty} Instance </p>
     */
    private final Map<Long, OutboundProperty> streamIdMap = new ConcurrentHashMap<>();

    @Override
    protected void handlerAdded0(ChannelHandlerContext ctx) throws Exception {
        FRAME_CODEC_INSTANCE = getHttp2FrameCodec();

        try {
            Class<?> clazz = Class.forName("io.netty.handler.codec.http2.Http2FrameCodec$DefaultHttp2FrameStream");
            FRAME_CODEC_METHOD = Http2FrameCodec.class.getDeclaredMethod("initializeNewStream", ChannelHandlerContext.class, clazz, ChannelPromise.class);
            FRAME_CODEC_METHOD.setAccessible(true);
        } catch (ClassNotFoundException | NoSuchMethodException ex) {
            logger.error("Failed to initialize method 'Http2FrameCodec#initializeNewStream'", ex);
            throw ex;
        }
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpFrame httpFrame = (HttpFrame) msg;

            Http2FrameStream http2FrameStream = newStream();

            // Invoke and initialize new 'Http2FrameStream'
            invokeInitializeNewStream(ctx, http2FrameStream, promise);
            long id = httpFrame.id();

            // Put the stream ID and Outbound Property into the map.
            addStream(new OutboundProperty(id, http2FrameStream, httpFrame.protocol()));

            if (msg instanceof FullHttpRequest fullHttpRequest) {

                if (fullHttpRequest.content().readableBytes() == 0) {
                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(fullHttpRequest);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, id, http2HeadersFrame, promise);
                } else {
                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(fullHttpRequest);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    writeHeaders(ctx, id, http2HeadersFrame, promise);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                    writeData(ctx, id, dataFrame, ctx.voidPromise());
                }
            } else {
                Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(((HttpRequest) msg));
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, id, http2HeadersFrame, promise);
            }
        } else if (msg instanceof HttpContent httpContent) {
            long id = ((HttpFrame) httpContent).id();

            if (msg instanceof LastHttpContent) {
                LastHttpContent lastHttpContent = (LastHttpContent) httpContent;

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                // which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    writeData(ctx, id, dataFrame, promise);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    writeData(ctx, id, dataFrame, promise);

                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders());
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, id, http2HeadersFrame, promise);
                }
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, id, dataFrame, promise);
            }
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame headersFrame) {
            int streamId = headersFrame.stream().id();
            OutboundProperty outboundProperty = outboundProperty(streamId);

            HttpVersion httpVersion;
            if (outboundProperty.protocol().equals(HttpFrame.Protocol.HTTP_1_0)) {
                httpVersion = HttpVersion.HTTP_1_0;
            } else {
                httpVersion = HttpVersion.HTTP_1_1;
            }

            // If initial read is already performed then this Header frame is part of Last trailing frame.
            if (outboundProperty.initialRead()) {
                LastHttpContent httpContent = new CustomLastHttpContent(Unpooled.EMPTY_BUFFER, outboundProperty.protocol(), outboundProperty.id());

                HTTPConversionUtil.addHttp2ToHttpHeaders(headersFrame.headers(), httpContent.trailingHeaders(), true, false);
                ctx.fireChannelRead(httpContent);
                removeStreamMapping(streamId);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                outboundProperty.fireInitialRead();

                HttpResponse httpResponse;

                // If 'endOfStream' flag is set to 'true' then we will create FullHttpResponse and remove mapping.
                if (headersFrame.isEndStream()) {
                    httpResponse = HTTPConversionUtil.toFullHttpResponse(outboundProperty.id(), headersFrame.headers(), Unpooled.EMPTY_BUFFER, httpVersion);
                    removeStreamMapping(streamId);
                } else {
                    httpResponse = HTTPConversionUtil.toHttpResponse(outboundProperty.id(), headersFrame.headers(), httpVersion);
                    httpResponse.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
                }

                ctx.fireChannelRead(httpResponse);
            }
        } else if (msg instanceof Http2DataFrame dataFrame) {
            int streamId = dataFrame.stream().id();
            OutboundProperty outboundProperty = outboundProperty(streamId);

            HttpContent httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new CustomLastHttpContent(dataFrame.content(), outboundProperty.protocol(), outboundProperty.id());
                removeStreamMapping(streamId);
            } else {
                httpContent = new CustomHttpContent(dataFrame.content(), outboundProperty.protocol(), outboundProperty.id());
            }

            ctx.fireChannelRead(httpContent);
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, long id, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) throws Exception {
        headersFrame.stream(outboundProperty(id).stream());
        super.write(ctx, headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, long id, Http2DataFrame dataFrame, ChannelPromise channelPromise) throws Exception {
        dataFrame.stream(outboundProperty(id).stream());
        super.write(ctx, dataFrame, channelPromise);
    }

    private void addStream(OutboundProperty outboundProperty) {
        requestIdToStreamIdMap.put(outboundProperty.stream().id(), outboundProperty.id());
        streamIdMap.put(outboundProperty.id(), outboundProperty);
    }

    private OutboundProperty outboundProperty(int streamId) {
        return outboundProperty(requestIdToStreamIdMap.get(streamId));
    }

    /**
     * Get {@linkplain OutboundProperty} using {@code ID}
     */
    private OutboundProperty outboundProperty(long id) {
        OutboundProperty outboundProperty = streamIdMap.get(id);
        if (outboundProperty == null) {
            throw new IllegalArgumentException("Stream does not exist for StreamHash: " + id);
        }
        return outboundProperty;
    }

    private void removeStreamMapping(int streamId) {
        streamIdMap.remove(requestIdToStreamIdMap.remove(streamId));
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at HTTP2OutboundAdapter", cause);
    }

    private Http2FrameCodec getHttp2FrameCodec() {
        try {
            Field http2FrameCodecField = HTTP2OutboundAdapter.class.getSuperclass().getDeclaredField("frameCodec");
            http2FrameCodecField.setAccessible(true);
            return (Http2FrameCodec) http2FrameCodecField.get(this);
        } catch (Exception ex) {
            logger.error("Failed to access 'frameCodec' instance", ex);
            return null;
        }
    }

    private void invokeInitializeNewStream(ChannelHandlerContext ctx, Http2FrameStream stream, ChannelPromise promise) {
        try {
            FRAME_CODEC_METHOD.invoke(FRAME_CODEC_INSTANCE, ctx, stream, promise);
        } catch (InvocationTargetException | IllegalAccessException e) {
            logger.error("Failed to invoke 'Http2FrameCodec#initializeNewStream'", e);
        }
    }
}
