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
import com.shieldblaze.expressgateway.protocol.http.compression.HTTP2ContentDecompressor;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionDecoder;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2FrameReader;
import io.netty.handler.codec.http2.DefaultHttp2FrameWriter;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersDecoder;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameReader;
import io.netty.handler.codec.http2.Http2FrameWriter;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersEncoder;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PromisedRequestVerifier;
import io.netty.handler.codec.http2.Http2Settings;
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTP2OutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(
                newCodec(),
                new ChannelDuplexHandler() {
                    @Override
                    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
                        if (msg instanceof Http2HeadersFrame) {
                            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
                            assertTrue(headersFrame.isEndStream());
                            assertEquals(3, headersFrame.stream().id());

                            Http2Headers http2Headers = new DefaultHttp2Headers();
                            http2Headers.status("200");
                            http2Headers.set("shieldblaze", "expressgateway");

                            Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                            responseHeadersFrame.stream(headersFrame.stream());
                            ctx.fireChannelRead(responseHeadersFrame);

                            Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("Meow".getBytes()), true);
                            http2DataFrame.stream(headersFrame.stream());
                            ctx.fireChannelRead(http2DataFrame);

                            return;
                        }
                        throw new IllegalArgumentException("Unknown Object: " + msg);
                    }
                },
                new HTTP2OutboundAdapter());

        HttpRequest httpRequest = new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
        httpRequest.headers().set(Headers.STREAM_HASH, 1);
        httpRequest.headers().set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_2);
        httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        HttpResponse httpResponse = embeddedChannel.readInbound();
        assertEquals(200, httpResponse.status().code());
        assertEquals("expressgateway", httpResponse.headers().get("shieldblaze"));

        DefaultHttp2TranslatedLastHttpContent httpContent = embeddedChannel.readInbound();
        assertEquals(1, httpContent.streamHash());
        assertEquals("Meow", new String(ByteBufUtil.getBytes(httpContent.content())));

        httpContent.release();
        embeddedChannel.close();
    }

    @Test
    void chunkedPOSTRequestAndResponse() {
        final int numBytesSend = 1024 * 100;
        final int numBytesReceived = 1024 * 200;

        EmbeddedChannel embeddedChannel = new EmbeddedChannel(
                newCodec(),
                new ChannelDuplexHandler() {
                    int i = 1;
                    int received = 0;

                    @Override
                    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
                        if (msg instanceof Http2HeadersFrame) {
                            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
                            assertFalse(headersFrame.isEndStream());
                            assertEquals(3, headersFrame.stream().id());

                            Http2Headers http2Headers = new DefaultHttp2Headers();
                            http2Headers.status("200");
                            http2Headers.set("shieldblaze", "expressgateway");

                            Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                            responseHeadersFrame.stream(headersFrame.stream());
                            ctx.fireChannelRead(responseHeadersFrame);

                            return;
                        } else if (msg instanceof Http2DataFrame) {
                            Http2DataFrame dataFrame = (Http2DataFrame) msg;
                            assertEquals("MeowSent" + ++received, new String(ByteBufUtil.getBytes(dataFrame.content())));
                            dataFrame.release();

                            // When all bytes are received then we'll start sending.
                            if (received == numBytesSend) {
                                for (i = 1; i <= numBytesReceived; i++) {
                                    Http2DataFrame http2DataFrame;

                                    ByteBuf byteBuf = Unpooled.wrappedBuffer(("MeowReceived" + i).getBytes());
                                    if (i == numBytesReceived) {
                                        http2DataFrame = new DefaultHttp2DataFrame(byteBuf, true);
                                    } else {
                                        http2DataFrame = new DefaultHttp2DataFrame(byteBuf, false);
                                    }
                                    http2DataFrame.stream(dataFrame.stream());
                                    ctx.fireChannelRead(http2DataFrame);
                                }
                            }

                            return;
                        }
                        throw new IllegalArgumentException("Unknown Object: " + msg);
                    }
                },
                new HTTP2OutboundAdapter());

        HttpRequest httpRequest = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/");
        httpRequest.headers().set(Headers.STREAM_HASH, 1);
        httpRequest.headers().set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_2);
        httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
        httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        HttpResponse httpResponse = embeddedChannel.readInbound();
        assertEquals(200, httpResponse.status().code());
        assertEquals("expressgateway", httpResponse.headers().get("shieldblaze"));

        // Send bytes
        for (int i = 1; i <= numBytesSend; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            ByteBuf byteBuf = Unpooled.wrappedBuffer(("MeowSent" + i).getBytes());
            HttpContent httpContent;
            if (i == numBytesSend) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(byteBuf, 1);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(byteBuf, 1);
            }

            embeddedChannel.writeOutbound(httpContent);
            embeddedChannel.flushOutbound();
        }

        // Receive bytes
        for (int i = 1; i <= numBytesReceived; i++) {
            Http2TranslatedHttpContent httpContent = embeddedChannel.readInbound();

            if (i == numBytesReceived) {
                assertTrue(httpContent instanceof DefaultHttp2TranslatedLastHttpContent);
            } else {
                assertTrue(httpContent instanceof DefaultHttp2TranslatedHttpContent);
            }

            assertEquals("MeowReceived" + i, new String(ByteBufUtil.getBytes(httpContent.content())));
            assertEquals(1, httpContent.streamHash());
            httpContent.release();
        }

        // Since we've read all HTTP Content, we don't have anything else left to read.
        // Calling readInbound must return null.
        assertNull(embeddedChannel.readInbound());

        embeddedChannel.close();
    }

    Http2FrameCodec newCodec() {
        Http2Settings http2Settings = Http2Settings.defaultSettings();
        Http2Connection connection = new DefaultHttp2Connection(false);

        Http2FrameReader reader = new DefaultHttp2FrameReader(new DefaultHttp2HeadersDecoder(true, http2Settings.maxHeaderListSize()));
        Http2FrameWriter writer = new DefaultHttp2FrameWriter(Http2HeadersEncoder.NEVER_SENSITIVE, false);

        Http2ConnectionEncoder encoder = new DefaultHttp2ConnectionEncoder(connection, writer);
        DefaultHttp2ConnectionDecoder decoder = new DefaultHttp2ConnectionDecoder(connection, encoder, reader, Http2PromisedRequestVerifier.ALWAYS_VERIFY,
                true, true);

        Http2FrameCodec http2FrameCodec = new Http2FrameCodec(encoder, decoder, http2Settings, false);
        decoder.frameListener(new HTTP2ContentDecompressor(connection, decoder.frameListener()));
        return http2FrameCodec;
    }
}
