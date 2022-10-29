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
import com.shieldblaze.expressgateway.protocol.http.compression.HTTP2ContentCompressor;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPCompressionUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelDuplexHandler;
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
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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

    private Http2FrameStream frameStream;
    private String acceptEncoding;

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame headersFrame) {
            onHttp2HeadersRead(ctx, headersFrame);
        } else if (msg instanceof Http2DataFrame dataFrame) {
            onHttp2DataRead(ctx, dataFrame);
        } else {
            // Unsupported message type
        }
    }

    private void bound(Http2FrameStream frameStream, String acceptEncoding) {
        this.frameStream = frameStream;
        this.acceptEncoding = acceptEncoding;
    }

    private void reset() {
        this.frameStream = null;
        this.acceptEncoding = null;
    }

    private void onHttp2HeadersRead(ChannelHandlerContext ctx, Http2HeadersFrame headersFrame) {
        if (frameStream != null) {
            LastHttpContent httpContent = new DefaultLastHttpContent(Unpooled.EMPTY_BUFFER, true);
            HTTPConversionUtil.addHttp2ToHttpHeaders(headersFrame.headers(), httpContent.trailingHeaders(), true, true);
            ctx.fireChannelRead(httpContent);

            // Trailing Header must have 'endOfStream' flag set to 'true'. If not, we'll send GOAWAY frame.
            if (!headersFrame.isEndStream()) {
                ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.PROTOCOL_ERROR).setExtraStreamIds(headersFrame.stream().id()), ctx.voidPromise());
                reset();
            }
        } else {
            HttpRequest httpRequest;
            if (headersFrame.isEndStream()) {
                httpRequest = HTTPConversionUtil.toFullHttpRequestNormal(headersFrame.headers(), Unpooled.EMPTY_BUFFER);
            } else {
                httpRequest = HTTPConversionUtil.toHttpRequestNormal(headersFrame.headers());
                httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
            }

            if (headersFrame.headers().contains(HttpHeaderNames.ACCEPT_ENCODING)) {
                acceptEncoding = headersFrame.headers().get(HttpHeaderNames.ACCEPT_ENCODING).toString();
            } else {
                acceptEncoding = null;
            }

            bound(headersFrame.stream(), acceptEncoding);
            ctx.fireChannelRead(httpRequest);
        }
    }

    private void onHttp2DataRead(ChannelHandlerContext ctx, Http2DataFrame dataFrame) {
        HttpContent httpContent;
        if (dataFrame.isEndStream()) {
            httpContent = new DefaultLastHttpContent(dataFrame.content());
        } else {
            httpContent = new DefaultHttpContent(dataFrame.content());
        }
        ctx.fireChannelRead(httpContent);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        if (msg instanceof FullHttpResponse fullHttpResponse) {
            Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(fullHttpResponse);

            applyCompression(http2Headers);

            // If 'readableBytes' is 0 then there is no Data frame to write. We'll mark Header frame as 'endOfStream'.
            if (fullHttpResponse.content().readableBytes() == 0) {
                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                writeHeaders(ctx, headersFrame, promise);
            } else {
                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                writeHeaders(ctx, headersFrame, promise);

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpResponse.content(), true);
                writeData(ctx, dataFrame, ctx.voidPromise(), true);
            }

            // We're done with this Stream
            reset();
        } else if (msg instanceof HttpResponse httpResponse) {
            Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(httpResponse);
            applyCompression(http2Headers);

            Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
            writeHeaders(ctx, headersFrame, promise);
        } else if (msg instanceof HttpContent httpContent) {
            if (httpContent instanceof LastHttpContent lastHttpContent) {
                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame
                //   which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), true);
                    writeData(ctx, dataFrame, promise, false);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(lastHttpContent.content(), false);
                    writeData(ctx, dataFrame, promise, false);

                    Http2Headers http2Headers = HTTPConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders());
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    writeHeaders(ctx, headersFrame, ctx.voidPromise());
                }

                // We're done with this Stream
                reset();
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                writeData(ctx, dataFrame, promise, false);
            }
        }
    }

    /**
     * <p> Determine whether compression can be applied or not. </p>
     *
     * <p> If {@link Http2Headers} does not contain 'CONTENT-ENCODING' and
     * {@link #acceptEncoding} is not 'null' then
     * we'll call {@link HTTPCompressionUtil#targetEncoding(Http2Headers, String)}
     * to determine whether the content is compressible or not.
     * If content is compressible then we'll add 'CONTENT-ENCODING' headers so it
     * can be compressed by {@link HTTP2ContentCompressor}. </p>
     */
    private void applyCompression(Http2Headers headers) {
        if (!headers.contains(HttpHeaderNames.CONTENT_ENCODING) && acceptEncoding != null) {
            String targetEncoding = HTTPCompressionUtil.targetEncoding(headers, acceptEncoding);
            if (targetEncoding != null) {
                headers.set(HttpHeaderNames.CONTENT_ENCODING, targetEncoding);
            }
        }
    }

    /**
     * Write {@linkplain Http2HeadersFrame}
     */
    private void writeHeaders(ChannelHandlerContext ctx, Http2HeadersFrame headersFrame, ChannelPromise channelPromise) {
        headersFrame.stream(frameStream);
        ctx.write(headersFrame, channelPromise);
    }

    /**
     * Write and Flush {@linkplain Http2DataFrame}
     */
    private void writeData(ChannelHandlerContext ctx, Http2DataFrame dataFrame, ChannelPromise channelPromise, boolean flush) {
        dataFrame.stream(frameStream);
        if (flush) {
            ctx.writeAndFlush(dataFrame, channelPromise);
        } else {
            ctx.write(dataFrame, channelPromise);
        }
    }

    public Http2FrameStream frameStream() {
        return frameStream;
    }

    public String acceptEncoding() {
        return acceptEncoding;
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught error at HTTP2InboundAdapter", cause);
    }
}
