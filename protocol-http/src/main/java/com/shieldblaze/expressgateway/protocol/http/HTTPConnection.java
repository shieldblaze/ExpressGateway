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

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Exception;
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
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslHandler;
import io.netty.util.ReferenceCountUtil;

import static io.netty.handler.codec.http.HttpHeaderNames.ACCEPT_ENCODING;

final class HttpConnection extends Connection {

    private final StreamPropertyMap MAP = new StreamPropertyMap();
    final HttpConfiguration httpConfiguration;
    private StreamPropertyMap.StreamProperty lastTranslatedStreamProperty;

    /**
     * Set to {@code true} if this connection is established on top of HTTP/2 (h2)
     */
    private boolean isConnectionHttp2;

    @NonNull
    HttpConnection(Node node, HttpConfiguration httpConfiguration) {
        super(node);
        this.httpConfiguration = httpConfiguration;
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        ALPNHandler alpnHandler = channelFuture.channel().pipeline().get(ALPNHandler.class);

        // If operation was successful then we'll check if ALPNHandler is available or not.
        // If ALPNHandler is available (not null) then we'll see if ALPN has negotiated HTTP/2 or not.
        // We'll then write the backlog or clear the backlog.
        if (channelFuture.isSuccess()) {
            if (alpnHandler != null) {
                alpnHandler.protocol().whenCompleteAsync((protocol, throwable) -> {

                    // If throwable is 'null' then task is completed successfully without any error.
                    if (throwable == null) {
                        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                            isConnectionHttp2 = true;
                        }
                        writeBacklog();
                    } else {
                        clearBacklog();
                    }
                }, channel.eventLoop());
            } else {
                writeBacklog();
            }
        } else {
            clearBacklog();
        }
    }

    @Override
    protected void writeBacklog() {
        for (Object o : backlogQueue) {
            try {
                writeIntoChannel(o);
            } catch (Exception ex) {
                throw new IllegalStateException(ex);
            }
        }
        backlogQueue.clear(); // Clear the new queue because we're done with it.
    }

    @NonNull
    @Override
    public void writeAndFlush(Object o) {
        if (state == State.INITIALIZED) {
            backlogQueue.add(o);
        } else if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            try {
                writeIntoChannel(o);
            } catch (Exception ex) {
                throw new IllegalStateException(ex);
            }
        } else {
            ReferenceCountUtil.release(o);
        }
    }

    private void writeIntoChannel(Object o) throws Http2Exception {
        // If connection protocol is HTTP/2 and request is HTTP/1.1 then convert the request to HTTP/2.
        //
        // If connection protocol is HTTP/2 and request is HTTP/2 then proxy it.
        if (o instanceof HttpRequest || o instanceof HttpContent) {
            if (isConnectionHttp2) {
                proxyOutboundHttp11ToHttp2(o);
            } else {

                // Apply compression
                if (o instanceof HttpRequest httpRequest) {
                    applySupportedCompressionHeaders(httpRequest.headers());
                }

                // Proxy HTTP/1.1 requests without any modification
                channel.writeAndFlush(o);
            }
        } else if (o instanceof Http2HeadersFrame || o instanceof Http2DataFrame) {
            if (isConnectionHttp2) {
                proxyOutboundHttp2ToHttp2((Http2StreamFrame) o);
            } else {
                proxyOutboundHttp2ToHttp11(o);
            }
        } else if (o instanceof Http2SettingsFrame || o instanceof Http2PingFrame || o instanceof Http2SettingsAckFrame) {
            channel.writeAndFlush(o);
        } else if (o instanceof Http2GoAwayFrame goAwayFrame) {

            // If Connection is HTTP/2 then send GOAWAY to server.
            // Else close the HTTP/1.1 connection.
            if (isConnectionHttp2) {
                System.exit(1);
                Http2GoAwayFrame http2GoAwayFrame = new DefaultHttp2GoAwayFrame(goAwayFrame.errorCode(), goAwayFrame.content());
                http2GoAwayFrame.setExtraStreamIds(goAwayFrame.lastStreamId());
                streamPropertyMap().remove(goAwayFrame.lastStreamId());

                channel.writeAndFlush(goAwayFrame);
            } else {
                ReferenceCountUtil.release(goAwayFrame);
                close();
            }
        } else if (o instanceof Http2WindowUpdateFrame windowUpdateFrame) {
            if (isConnectionHttp2) {
                StreamPropertyMap.StreamProperty streamProperty = streamPropertyMap().get(windowUpdateFrame.stream());
                windowUpdateFrame.stream(streamProperty.proxyFrameStream());

                channel.writeAndFlush(windowUpdateFrame);
            }
        } else if (o instanceof Http2ResetFrame http2ResetFrame) {
            if (isConnectionHttp2) {
                http2ResetFrame.stream(streamPropertyMap().remove(http2ResetFrame.stream()).proxyFrameStream());
                channel.writeAndFlush(http2ResetFrame);
            }
        } else if (o instanceof WebSocketFrame) {
            channel.writeAndFlush(o);
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException(o);
        }
    }

    private void proxyOutboundHttp2ToHttp11(Object o) throws Http2Exception {
        if (o instanceof Http2HeadersFrame headersFrame) {
            // Apply compression
            applySupportedCompressionHeaders(headersFrame.headers());

            if (headersFrame.isEndStream()) {
                FullHttpRequest fullHttpRequest = HttpConversionUtil.toFullHttpRequest(-1, headersFrame.headers(), Unpooled.EMPTY_BUFFER, true);
                channel.writeAndFlush(fullHttpRequest);

                clearTranslatedStreamProperty();
            } else {
                HttpRequest httpRequest = HttpConversionUtil.toHttpRequest(-1, headersFrame.headers(), true);
                channel.writeAndFlush(httpRequest);

                lastTranslatedStreamProperty = new StreamPropertyMap.StreamProperty(httpRequest.headers().get(ACCEPT_ENCODING),
                        headersFrame.stream(), headersFrame.stream());
            }
        } else if (o instanceof Http2DataFrame dataFrame) {
            lastTranslatedStreamProperty = new StreamPropertyMap.StreamProperty(null, dataFrame.stream(), dataFrame.stream());

            if (dataFrame.isEndStream()) {
                LastHttpContent lastHttpContent = new DefaultLastHttpContent(dataFrame.content());
                channel.writeAndFlush(lastHttpContent);
            } else {
                HttpContent httpContent = new DefaultHttpContent(dataFrame.content());
                channel.writeAndFlush(httpContent);
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException(o);
        }
    }

    /**
     * Proxy {@link Http2HeadersFrame} and {@link Http2DataFrame}
     */
    private void proxyOutboundHttp2ToHttp2(Http2StreamFrame streamFrame) {
        if (streamFrame instanceof Http2HeadersFrame headersFrame) {
            // Apply compression
            CharSequence clientAcceptEncoding = headersFrame.headers().get(ACCEPT_ENCODING);
            applySupportedCompressionHeaders(headersFrame.headers());

            Http2FrameStream clientFrameStream = headersFrame.stream();
            System.out.println(streamFrame);
            Http2FrameStream proxyFrameStream = newFrameStream(clientFrameStream.id());
            headersFrame.stream(proxyFrameStream);
            channel.writeAndFlush(headersFrame);

            streamPropertyMap().put(proxyFrameStream.id(), new StreamPropertyMap.StreamProperty(String.valueOf(clientAcceptEncoding), clientFrameStream, proxyFrameStream));
        } else if (streamFrame instanceof Http2DataFrame dataFrame) {
            Http2FrameStream oldFrameStream = dataFrame.stream();
            dataFrame.stream(streamPropertyMap().get(oldFrameStream).proxyFrameStream());

            channel.writeAndFlush(dataFrame);
        }
    }

    /**
     * Proxy {@link HttpRequest} and {@link HttpContent}
     */
    private void proxyOutboundHttp11ToHttp2(Object o) {
        if (o instanceof HttpRequest httpRequest) {
            String clientAcceptEncoding = httpRequest.headers().get(ACCEPT_ENCODING);
            boolean isTLSConnection = channel.pipeline().get(SslHandler.class) != null;
            Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpRequest, true);
            http2Headers.scheme(isTLSConnection ? "https" : "http");
            Http2FrameStream frameStream = newFrameStream();

            // Apply compression
            applySupportedCompressionHeaders(http2Headers);

            if (httpRequest instanceof FullHttpRequest fullHttpRequest) {
                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                headersFrame.stream(frameStream);

                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                dataFrame.stream(frameStream);

                channel.write(headersFrame);
                channel.writeAndFlush(dataFrame);
            } else {
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                http2HeadersFrame.stream(frameStream);
                channel.writeAndFlush(http2HeadersFrame);

                // There are HttpContent in queue to process, so we will store this FrameStream for further use.
                lastTranslatedStreamProperty = new StreamPropertyMap.StreamProperty(clientAcceptEncoding, frameStream, frameStream);
            }
        } else if (o instanceof HttpContent httpContent) {
            if (httpContent instanceof LastHttpContent lastHttpContent) {

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    dataFrame.stream(lastTranslatedStreamProperty.proxyFrameStream());
                    clearTranslatedStreamProperty();

                    channel.writeAndFlush(dataFrame);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    dataFrame.stream(lastTranslatedStreamProperty.proxyFrameStream());

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), true);
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    headersFrame.stream(lastTranslatedStreamProperty.proxyFrameStream());
                    clearTranslatedStreamProperty();

                    channel.write(headersFrame);
                    channel.writeAndFlush(dataFrame);
                }
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                dataFrame.stream(lastTranslatedStreamProperty.proxyFrameStream());
                channel.writeAndFlush(dataFrame);
            }
        }
    }

    private static void applySupportedCompressionHeaders(Object o) {
        // Set supported compression headers
        if (o instanceof HttpHeaders headers) {
            headers.set(ACCEPT_ENCODING, "br, gzip, deflate");
        } else if (o instanceof Http2Headers headers) {
            headers.set(ACCEPT_ENCODING, "br, gzip, deflate");
        }
    }

    private Http2FrameStream newFrameStream() {
        return channel.pipeline().get(Http2ChannelDuplexHandler.class).newStream();
    }

    private Http2FrameStream newFrameStream(int streamId) {
        return channel.pipeline().get(Http2ChannelDuplexHandler.class).newStream(streamId);
    }

    public StreamPropertyMap.StreamProperty lastTranslatedStreamProperty() {
        return lastTranslatedStreamProperty;
    }

    public void clearTranslatedStreamProperty() {
        lastTranslatedStreamProperty = null;
    }

    public StreamPropertyMap streamPropertyMap() {
        return MAP;
    }

    @Override
    public String toString() {
        return "HTTPConnection{" + "isConnectionHttp2=" + isConnectionHttp2 + ", Connection=" + super.toString() + '}';
    }
}
