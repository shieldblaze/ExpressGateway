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

import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufAllocator;
import io.netty.buffer.EmptyByteBuf;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import io.netty.handler.codec.http.FullHttpMessage;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMessage;
import io.netty.handler.codec.http.HttpObject;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.EmptyHttp2Headers;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.util.ReferenceCountUtil;

import java.nio.charset.StandardCharsets;

import static io.netty.handler.codec.http2.Http2Error.PROTOCOL_ERROR;
import static io.netty.handler.codec.http2.Http2Exception.connectionError;

final class DuplexHTTP2ToHTTPObjectAdapter extends ChannelDuplexHandler {

    private boolean isConnectionInitiated;
    private Http2FrameStream http2FrameStream;

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof DefaultHttp2HeadersFrame) {
            DefaultHttp2HeadersFrame http2HeadersFrame = (DefaultHttp2HeadersFrame) msg;
            onHeadersRead(ctx, http2HeadersFrame.stream().id(), http2HeadersFrame.headers(), http2HeadersFrame.padding(), http2HeadersFrame.isEndStream());
            if (http2FrameStream == null) {
                http2FrameStream = http2HeadersFrame.stream();
            }
        } else if (msg instanceof DefaultHttp2DataFrame) {
            DefaultHttp2DataFrame http2DataFrame = (DefaultHttp2DataFrame) msg;
            onDataRead(ctx, http2DataFrame.stream().id(), http2DataFrame.content(), http2DataFrame.padding(), http2DataFrame.isEndStream());
        }
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {

        // We only accept HttpMessage and HttpContent.
        if (!(msg instanceof HttpMessage || msg instanceof HttpContent)) {
            return;
        }

        boolean release = true;

        try {

            boolean endStream = false;

            if (msg instanceof HttpMessage) {
                HttpMessage httpMsg = (HttpMessage) msg;

                // Convert and write the headers.
                Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpMsg, true);
                endStream = msg instanceof FullHttpMessage && !((FullHttpMessage) msg).content().isReadable();

                writeHeaders(ctx, http2Headers, endStream);
            }

            if (!endStream && msg instanceof HttpContent) {
                boolean isLastContent = false;
                HttpHeaders trailers = EmptyHttpHeaders.INSTANCE;
                Http2Headers http2Trailers = EmptyHttp2Headers.INSTANCE;
                if (msg instanceof LastHttpContent) {
                    isLastContent = true;

                    // Convert any trailing headers.
                    final LastHttpContent lastContent = (LastHttpContent) msg;
                    trailers = lastContent.trailingHeaders();
                    http2Trailers = HttpConversionUtil.toHttp2Headers(trailers, true);
                }

                // Write the data
                final ByteBuf content = ((HttpContent) msg).content();
                endStream = isLastContent && trailers.isEmpty();

                writeData(ctx, content, endStream);
                release = false;

                if (!trailers.isEmpty()) {
                    // Write trailing headers.
                    writeHeaders(ctx, http2Trailers, endStream);
                }
            }
        } catch (Throwable t) {
            promise.setFailure(t);
        } finally {
            if (release) {
                ReferenceCountUtil.release(msg);
            }
            promise.setSuccess();
        }
    }

    private void writeHeaders(ChannelHandlerContext ctx, Http2Headers http2Headers, boolean endStream) {
        DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, endStream);
        defaultHttp2HeadersFrame.stream(http2FrameStream);
        ctx.writeAndFlush(defaultHttp2HeadersFrame);
    }

    private void writeData(ChannelHandlerContext ctx, ByteBuf data, boolean endStream) {
        DefaultHttp2DataFrame defaultHttp2DataFrame = new DefaultHttp2DataFrame(data, endStream);
        defaultHttp2DataFrame.stream(http2FrameStream);
        ctx.writeAndFlush(defaultHttp2DataFrame);
    }

    private void onHeadersRead(ChannelHandlerContext ctx, int streamId, Http2Headers headers, int padding, boolean endOfStream) throws Http2Exception {
        HttpMessage msg = processHeadersBegin(ctx, streamId, headers, endOfStream, true, true);
        if (msg != null) {
            processHeadersEnd(ctx, msg, endOfStream);
        }
    }

    private void onDataRead(ChannelHandlerContext ctx, int streamId, ByteBuf data, int padding, boolean endOfStream) throws Http2Exception {
        if (!isConnectionInitiated) {
            throw connectionError(PROTOCOL_ERROR, "Data Frame received for unknown Stream ID %d", streamId);
        }

        ByteBuf content = ctx.alloc().buffer();
        content.writeBytes(data);

        if (endOfStream) {
            if (content.readableBytes() != 0) {
                fireChannelRead(ctx, new DefaultHttp2TranslatedHttpContent(content, streamId));
            }
            fireChannelRead(ctx, new DefaultHttp2TranslatedLastHttpContent(streamId));
        } else {
            fireChannelRead(ctx, new DefaultHttp2TranslatedHttpContent(content, streamId));
        }
    }

    private HttpMessage newMessage(int streamId, Http2Headers headers, ByteBufAllocator alloc, boolean endOfStream) throws Http2Exception {
        if (endOfStream) {
            return HttpConversionUtil.toFullHttpRequest(streamId, headers, alloc, true);
        } else {
            HttpRequest httpRequest = HttpConversionUtil.toHttpRequest(streamId, headers, true);
            httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
            return httpRequest;
        }
    }

    private HttpMessage processHeadersBegin(ChannelHandlerContext ctx, int streamId, Http2Headers headers, boolean endOfStream,
                                            boolean allowAppend, boolean appendToTrailer) throws Http2Exception {
        if (!isConnectionInitiated) {
            return newMessage(streamId, headers, ctx.alloc(), endOfStream);
        } else if (allowAppend) {
            DefaultLastHttpContent defaultLastHttpContent = new DefaultLastHttpContent(new EmptyByteBuf(ctx.alloc()), false);
            HttpHeaders httpHeaders = defaultLastHttpContent.trailingHeaders();
            HttpConversionUtil.addHttp2ToHttpHeaders(streamId, headers, httpHeaders, HttpVersion.HTTP_1_1, appendToTrailer, true);
        }

        return null;
    }

    private void processHeadersEnd(ChannelHandlerContext ctx, HttpObject msg, boolean endOfStream) {
        fireChannelRead(ctx, msg);
        if (!endOfStream) {
            isConnectionInitiated = true;
        }
    }

    protected void fireChannelRead(ChannelHandlerContext ctx, HttpObject msg) {
        if (msg instanceof FullHttpMessage) {
            HttpUtil.setContentLength((FullHttpMessage) msg, ((FullHttpMessage) msg).content().readableBytes());
        }
        ctx.fireChannelRead(msg);
    }
}
