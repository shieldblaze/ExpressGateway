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
import com.shieldblaze.expressgateway.protocol.http.NonceWrapped;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultLastHttpContent;
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
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2FrameStreamEvent;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.ssl.SslHandler;
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
    private static final Method FRAME_CODEC_METHOD;

    static {
        // Cache the lookup
        Method method = null;
        try {
            Class<?> clazz = Class.forName("io.netty.handler.codec.http2.Http2FrameCodec$DefaultHttp2FrameStream");
            method = Http2FrameCodec.class.getDeclaredMethod("initializeNewStream", ChannelHandlerContext.class, clazz, ChannelPromise.class);
            method.setAccessible(true);
        } catch (ClassNotFoundException | NoSuchMethodException ex) {
            logger.error("Failed to initialize method 'Http2FrameCodec#initializeNewStream'", ex);
        }
        FRAME_CODEC_METHOD = method;
    }

    private Http2FrameCodec FRAME_CODEC_INSTANCE;

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

    private boolean isTLSConnection = false;

    @Override
    protected void handlerAdded0(ChannelHandlerContext ctx) {
        FRAME_CODEC_INSTANCE = getHttp2FrameCodec();
        isTLSConnection = ctx.pipeline().get(SslHandler.class) != null;
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        NonceWrapped<?> nonceWrapped = (NonceWrapped<?>) msg;
        if (nonceWrapped.get() instanceof HttpRequest httpRequest) {
            Http2FrameStream http2FrameStream = newStream();

            // Invoke and initialize new 'Http2FrameStream'
            invokeInitializeNewStream(ctx, http2FrameStream, promise);

            // Put the stream ID and Outbound Property into the map.
            addStream(new OutboundProperty(nonceWrapped.nonce(), http2FrameStream, HttpVersion.HTTP_1_1));

            if (httpRequest instanceof FullHttpRequest fullHttpRequest) {

                if (!fullHttpRequest.content().isReadable()) {
                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(fullHttpRequest);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, nonceWrapped.nonce(), http2HeadersFrame, promise, true);
                } else {
                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(fullHttpRequest);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    writeHeaders(ctx, nonceWrapped.nonce(), http2HeadersFrame, promise, true);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                    writeData(ctx, nonceWrapped.nonce(), dataFrame, ctx.voidPromise());
                }
            } else {
                Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(httpRequest);
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, nonceWrapped.nonce(), http2HeadersFrame, promise, true);
            }
        } else if (nonceWrapped.get() instanceof HttpContent httpContent) {
            if (httpContent instanceof LastHttpContent) {
                LastHttpContent lastHttpContent = (LastHttpContent) nonceWrapped.get();

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                //   which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    writeData(ctx, nonceWrapped.nonce(), dataFrame, promise);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    writeData(ctx, nonceWrapped.nonce(), dataFrame, promise);

                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders());
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, nonceWrapped.nonce(), http2HeadersFrame, promise, false);
                }
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, nonceWrapped.nonce(), dataFrame, promise);
            }
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame headersFrame) {
            int streamId = headersFrame.stream().id();
            OutboundProperty outboundProperty = outboundProperty(streamId);

            // If initial read is already performed then this Header frame is part of Last trailing frame.
            if (outboundProperty.initialRead()) {
                LastHttpContent httpContent = new DefaultLastHttpContent(Unpooled.EMPTY_BUFFER);

                HTTPConversionUtil.addHttp2ToHttpHeaders(headersFrame.headers(), httpContent.trailingHeaders(), true, false);
                ctx.fireChannelRead(httpContent);
                removeStreamMapping(streamId);

                // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
                if (!headersFrame.isEndStream()) {
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR).setExtraStreamIds(streamId));
                }
            } else {
                outboundProperty.fireInitialRead();

                NonceWrapped<HttpResponse> nonceWrapped;

                // If 'endOfStream' flag is set to 'true' then we will create FullHttpResponse and remove mapping.
                if (headersFrame.isEndStream()) {
                    nonceWrapped = HTTPConversionUtil.toFullHttpResponse(headersFrame.headers(), Unpooled.EMPTY_BUFFER, HttpVersion.HTTP_1_1);
                    removeStreamMapping(streamId);
                } else {
                    nonceWrapped = HTTPConversionUtil.toHttpResponse(headersFrame.headers(), HttpVersion.HTTP_1_1);
                    nonceWrapped.get().headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
                }

                ctx.fireChannelRead(nonceWrapped);
            }
        } else if (msg instanceof Http2DataFrame dataFrame) {
            int streamId = dataFrame.stream().id();
            OutboundProperty outboundProperty = outboundProperty(streamId);

            NonceWrapped<HttpContent> httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new NonceWrapped<>(outboundProperty.nonce(), new DefaultLastHttpContent(dataFrame.content()));
                removeStreamMapping(streamId);
            } else {
                httpContent = new NonceWrapped<>(outboundProperty.nonce(), new DefaultHttpContent(dataFrame.content()));
            }

            ctx.fireChannelRead(httpContent);
        }
    }

    /**
     * Write and Flush {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, long nonce, Http2HeadersFrame headersFrame,
                              ChannelPromise channelPromise, boolean addScheme) throws Exception {
        headersFrame.stream(outboundProperty(nonce).stream());
        if (addScheme) {
            headersFrame.headers().scheme(isTLSConnection ? "https" : "http");
        }
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
        requestIdToStreamIdMap.put(outboundProperty.stream().id(), outboundProperty.nonce());
        streamIdMap.put(outboundProperty.nonce(), outboundProperty);
    }

    private OutboundProperty outboundProperty(int streamId) {
        return outboundProperty(requestIdToStreamIdMap.get(streamId));
    }

    /**
     * Get {@linkplain OutboundProperty} using nonce
     */
    private OutboundProperty outboundProperty(long nonce) {
        OutboundProperty outboundProperty = streamIdMap.get(nonce);
        if (outboundProperty == null) {
            throw new IllegalArgumentException("Stream does not exist for Nonce: " + nonce);
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
