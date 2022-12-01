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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.protocol.http.compression.CompressionUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PingFrame;
import io.netty.handler.codec.http2.Http2ResetFrame;
import io.netty.handler.codec.http2.Http2SettingsAckFrame;
import io.netty.handler.codec.http2.Http2SettingsFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.Http2WindowUpdateFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;

import static io.netty.handler.codec.http2.HttpConversionUtil.toHttp2Headers;

final class DownstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final HttpConnection httpConnection;
    private final boolean isConnectionHttp2;
    private ChannelHandlerContext ctx;
    private Channel inboundChannel;
    private boolean headerRead;

    DownstreamHandler(HttpConnection httpConnection, Channel inboundChannel) {
        this.httpConnection = httpConnection;
        isConnectionHttp2 = inboundChannel.pipeline().get(Http2FrameCodec.class) != null;
        this.inboundChannel = inboundChannel;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (inboundChannel == null) {
            ReferenceCountUtil.release(msg);
        } else if (msg instanceof HttpResponse || msg instanceof HttpContent) {
            if (isConnectionHttp2) {
                normalizeInboundHttp11ToHttp2(msg);
            } else {
                inboundChannel.writeAndFlush(msg);
            }
        } else if (msg instanceof Http2HeadersFrame || msg instanceof Http2DataFrame) {
            if (isConnectionHttp2) {
                proxyInboundHttp2ToHttp2((Http2StreamFrame) msg);
            } else {
                normalizeInboundHttp2ToHttp11(msg);
            }
        } else if (msg instanceof Http2WindowUpdateFrame windowUpdateFrame) {
            if (isConnectionHttp2) {
                final int streamId = windowUpdateFrame.stream().id();
                Http2FrameStream frameStream = httpConnection.streamPropertyMap().get(streamId).clientStream();
                windowUpdateFrame.stream(frameStream);

                inboundChannel.writeAndFlush(windowUpdateFrame);
            }
        } else if (msg instanceof Http2SettingsFrame || msg instanceof Http2PingFrame || msg instanceof Http2SettingsAckFrame) {
            if (isConnectionHttp2) {
                inboundChannel.writeAndFlush(msg);
            }
        } else if (msg instanceof Http2GoAwayFrame goAwayFrame) {
            if (isConnectionHttp2) {
                Http2GoAwayFrame http2GoAwayFrame = new DefaultHttp2GoAwayFrame(goAwayFrame.errorCode(), goAwayFrame.content());
                http2GoAwayFrame.setExtraStreamIds(goAwayFrame.lastStreamId());
                inboundChannel.writeAndFlush(http2GoAwayFrame);

                // Try to remove the stream if it exists
                httpConnection.streamPropertyMap().remove(goAwayFrame.lastStreamId());
            }
        } else if (msg instanceof Http2ResetFrame http2ResetFrame) {
            if (isConnectionHttp2) {
                final int streamId = http2ResetFrame.stream().id();
                Http2FrameStream clientFrameStream = httpConnection.streamPropertyMap().remove(streamId).clientStream();
                http2ResetFrame.stream(clientFrameStream);

                inboundChannel.writeAndFlush(http2ResetFrame);
            }
        } else if (msg instanceof WebSocketFrame) {
            inboundChannel.writeAndFlush(msg);
        } else {
            ReferenceCountUtil.release(msg);
            throw new UnsupportedMessageTypeException(msg);
        }
    }

    private void normalizeInboundHttp2ToHttp11(Object o) throws Http2Exception {
        if (o instanceof Http2HeadersFrame headersFrame) {
            if (headersFrame.isEndStream()) {
                if (headerRead) {
                    HttpHeaders httpHeaders = new DefaultHttpHeaders(true);
                    HttpConversionUtil.addHttp2ToHttpHeaders(-1, headersFrame.headers(), httpHeaders, HttpVersion.HTTP_1_1, true, false);

                    LastHttpContent lastHttpContent = new DefaultLastHttpContent(Unpooled.EMPTY_BUFFER, httpHeaders);
                    inboundChannel.writeAndFlush(lastHttpContent);
                    headerRead = false;
                } else {
                    FullHttpResponse fullHttpResponse = HttpConversionUtil.toFullHttpResponse(-1, headersFrame.headers(),
                            Unpooled.EMPTY_BUFFER, true);
                    inboundChannel.writeAndFlush(fullHttpResponse);
                }
            } else {
                HttpResponse httpResponse = HttpConversionUtil.toHttpResponse(-1, headersFrame.headers(), true);
                inboundChannel.writeAndFlush(httpResponse);
                headerRead = true;
            }
        } else if (o instanceof Http2DataFrame dataFrame) {
            if (dataFrame.isEndStream()) {
                LastHttpContent lastHttpContent = new DefaultLastHttpContent(dataFrame.content());
                inboundChannel.writeAndFlush(lastHttpContent);
                headerRead = false;
            } else {
                HttpContent httpContent = new DefaultHttpContent(dataFrame.content());
                inboundChannel.writeAndFlush(httpContent);
            }
        } else {
            ReferenceCountedUtil.silentRelease(o);
            throw new UnsupportedMessageTypeException("Unsupported Message: " + o.getClass().getSimpleName(),
                    Http2HeadersFrame.class, Http2DataFrame.class);
        }
    }

    private void proxyInboundHttp2ToHttp2(Http2StreamFrame streamFrame) {
        final int streamId = streamFrame.stream().id();
        Streams.Stream stream = httpConnection.streamPropertyMap().get(streamId);

        if (stream == null) {
            ReferenceCountUtil.release(streamFrame);
            return;
        } else {
            streamFrame.stream(stream.clientStream());
        }

        if (streamFrame instanceof Http2HeadersFrame headersFrame) {
            applyCompressionOnHttp2(headersFrame.headers(), stream.acceptEncoding());

            if (headersFrame.isEndStream()) {
                httpConnection.streamPropertyMap().remove(streamId);
            }
        } else if (streamFrame instanceof Http2DataFrame dataFrame) {
            if (dataFrame.isEndStream()) {
                httpConnection.streamPropertyMap().remove(streamId);
            }
        }

        inboundChannel.writeAndFlush(streamFrame);
    }

    private void normalizeInboundHttp11ToHttp2(Object o) {
        if (o instanceof HttpResponse httpResponse) {
            Http2Headers http2Headers = toHttp2Headers(httpResponse.headers(), true);

            if (httpResponse instanceof FullHttpResponse fullHttpResponse) {
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                http2HeadersFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpResponse.content(), true);
                dataFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());
                httpConnection.clearTranslatedStreamProperty();

                inboundChannel.write(http2Headers);
                inboundChannel.writeAndFlush(dataFrame);
            } else {
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                http2HeadersFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());

                inboundChannel.writeAndFlush(http2HeadersFrame);
            }
        } else if (o instanceof HttpContent httpContent) {
            if (httpContent instanceof LastHttpContent lastHttpContent) {

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    dataFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());
                    httpConnection.clearTranslatedStreamProperty();

                    inboundChannel.writeAndFlush(dataFrame);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    dataFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), true);
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    http2HeadersFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());
                    httpConnection.clearTranslatedStreamProperty();

                    inboundChannel.write(dataFrame);
                    inboundChannel.writeAndFlush(http2HeadersFrame);
                }
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                dataFrame.stream(httpConnection.lastTranslatedStreamProperty().clientStream());
                inboundChannel.writeAndFlush(dataFrame);
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported");
        }
    }

    private void applyCompressionOnHttp2(Http2Headers headers, String acceptEncoding) {
        String targetEncoding = CompressionUtil.checkCompressibleForHttp2(headers, acceptEncoding, httpConnection.httpConfiguration.compressionThreshold());
        if (targetEncoding != null) {
            headers.set(HttpHeaderNames.CONTENT_ENCODING, targetEncoding);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        close();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        try {
            if (cause instanceof IOException) {
                // Swallow this harmless IOException
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            close();
        }
    }

    @Override
    public synchronized void close() {
        if (inboundChannel != null) {
            inboundChannel.close();
            inboundChannel = null;
        }

        if (ctx != null) {
            ctx.channel().close();
            ctx = null;
        }
    }
}
