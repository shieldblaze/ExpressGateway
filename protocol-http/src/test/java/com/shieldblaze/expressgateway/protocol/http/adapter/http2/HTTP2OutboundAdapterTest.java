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

import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.protocol.http.HTTPCodecs;
import com.shieldblaze.expressgateway.protocol.http.NonceWrapped;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.Random;

import static io.netty.handler.codec.http.HttpMethod.GET;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTP2OutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(
                HTTPCodecs.http2ClientCodec(HttpConfiguration.DEFAULT),
                new ChannelDuplexHandler() {
                    @Override
                    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
                        if (msg instanceof Http2HeadersFrame headersFrame) {
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

        NonceWrapped<HttpRequest> httpRequest = new NonceWrapped<>(25, new DefaultFullHttpRequest(HTTP_1_1, GET, "/", Unpooled.EMPTY_BUFFER));
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        NonceWrapped<HttpResponse> httpResponse = embeddedChannel.readInbound();
        assertEquals(200, httpResponse.get().status().code());
        assertEquals("expressgateway", httpResponse.get().headers().get("shieldblaze"));

        NonceWrapped<HttpContent> httpContent = embeddedChannel.readInbound();
        assertEquals(25, httpContent.nonce());
        assertEquals("Meow", new String(ByteBufUtil.getBytes(httpContent.get().content())));

        httpContent.get().release();
        embeddedChannel.close();
    }

    @Test
    void chunkedPOSTRequestAndResponse() {
        final int numBytesSend = 1024 * 100;
        final int numBytesReceived = 1024 * 200;

        EmbeddedChannel embeddedChannel = new EmbeddedChannel(
                HTTPCodecs.http2ClientCodec(HttpConfiguration.DEFAULT),
                new ChannelDuplexHandler() {
                    int i = 1;
                    int received = 0;

                    @Override
                    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
                        if (msg instanceof Http2HeadersFrame headersFrame) {
                            assertFalse(headersFrame.isEndStream());
                            assertEquals(3, headersFrame.stream().id());

                            Http2Headers http2Headers = new DefaultHttp2Headers();
                            http2Headers.status("200");
                            http2Headers.set("shieldblaze", "expressgateway");

                            Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                            responseHeadersFrame.stream(headersFrame.stream());
                            ctx.fireChannelRead(responseHeadersFrame);

                            return;
                        } else if (msg instanceof Http2DataFrame dataFrame) {
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

        NonceWrapped<HttpRequest> httpRequest = new NonceWrapped<>(25, new DefaultHttpRequest(HTTP_1_1, HttpMethod.POST, "/"));
        httpRequest.get().headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        NonceWrapped<HttpResponse> httpResponse = embeddedChannel.readInbound();
        assertEquals(200, httpResponse.get().status().code());
        assertEquals("expressgateway", httpResponse.get().headers().get("shieldblaze"));

        // Send bytes
        for (int i = 1; i <= numBytesSend; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            ByteBuf byteBuf = Unpooled.wrappedBuffer(("MeowSent" + i).getBytes());
            NonceWrapped<HttpContent> httpContent;
            if (i == numBytesSend) {
                httpContent = new NonceWrapped<>(25, new DefaultLastHttpContent(byteBuf));
            } else {
                httpContent = new NonceWrapped<>(25, new DefaultHttpContent(byteBuf));
            }

            embeddedChannel.writeOutbound(httpContent);
            embeddedChannel.flushOutbound();
        }

        // Receive bytes
        for (int i = 1; i <= numBytesReceived; i++) {
            NonceWrapped<HttpContent> httpContent = embeddedChannel.readInbound();

            if (i == numBytesReceived) {
                assertTrue(httpContent.get() instanceof LastHttpContent);
            }

            assertEquals("MeowReceived" + i, httpContent.get().content().toString(StandardCharsets.UTF_8));
            assertEquals(25, httpContent.nonce());
            httpContent.get().release();
        }

        // Since we've read all HTTP Content, we don't have anything else left to read.
        // Calling readInbound must return null.
        assertNull(embeddedChannel.readInbound());

        embeddedChannel.close();
    }
}
