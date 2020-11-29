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
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;

import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class HTTP2InboundAdapterTest {

    static EmbeddedChannel embeddedChannel;

    @BeforeAll
    static void setupEmbeddedChannel() {
        embeddedChannel = new EmbeddedChannel(new HTTP2InboundAdapter());
    }

    @AfterAll
    static void shutdownEmbeddedChannel() {
        embeddedChannel.close();
    }

    @Test
    void http2InboundTest() {
        {
            Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
            http2HeadersFrame.stream(new CustomHttp2FrameStream(2));
            http2HeadersFrame.headers().method("GET");
            http2HeadersFrame.headers().scheme("https");
            http2HeadersFrame.headers().path("/");
            http2HeadersFrame.headers().authority("localhost");

            embeddedChannel.writeInbound(http2HeadersFrame);
            embeddedChannel.flushInbound();
            FullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

            assertEquals("GET", fullHttpRequest.method().name());
            assertEquals("https", fullHttpRequest.headers().get("x-http2-scheme"));
            assertEquals("/", fullHttpRequest.uri());
            assertEquals("localhost", fullHttpRequest.headers().get("host"));
            assertEquals(String.valueOf(2), fullHttpRequest.headers().get("x-http2-stream-id"));
            assertTrue(fullHttpRequest.headers().contains(Headers.STREAM_HASH));
            assertEquals(Headers.Values.HTTP_2, fullHttpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

            fullHttpRequest.release();
        }

        {
            Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
            http2HeadersFrame.stream(new CustomHttp2FrameStream(3));
            http2HeadersFrame.headers().method("POST");
            http2HeadersFrame.headers().scheme("http");
            http2HeadersFrame.headers().path("/meow");
            http2HeadersFrame.headers().authority("www.shieldblaze.com");

            embeddedChannel.writeInbound(http2HeadersFrame);
            embeddedChannel.flushInbound();
            FullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

            assertEquals("POST", fullHttpRequest.method().name());
            assertEquals("http", fullHttpRequest.headers().get("x-http2-scheme"));
            assertEquals("/meow", fullHttpRequest.uri());
            assertEquals("www.shieldblaze.com", fullHttpRequest.headers().get("host"));
            assertEquals(String.valueOf(3), fullHttpRequest.headers().get("x-http2-stream-id"));
            assertTrue(fullHttpRequest.headers().contains(Headers.STREAM_HASH));
            assertEquals(Headers.Values.HTTP_2, fullHttpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

            fullHttpRequest.release();
        }

        {
            Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), false);
            http2HeadersFrame.stream(new CustomHttp2FrameStream(4));
            http2HeadersFrame.headers().method("POST");
            http2HeadersFrame.headers().scheme("http");
            http2HeadersFrame.headers().path("/meow");
            http2HeadersFrame.headers().authority("www.shieldblaze.com");

            embeddedChannel.writeInbound(http2HeadersFrame);
            embeddedChannel.flushInbound();
            HttpRequest httpRequest = embeddedChannel.readInbound();

            assertEquals("POST", httpRequest.method().name());
            assertEquals("http", httpRequest.headers().get("x-http2-scheme"));
            assertEquals("/meow", httpRequest.uri());
            assertEquals("www.shieldblaze.com", httpRequest.headers().get("host"));
            assertEquals(String.valueOf(4), httpRequest.headers().get("x-http2-stream-id"));
            assertTrue(httpRequest.headers().contains(Headers.STREAM_HASH));
            assertEquals(Headers.Values.HTTP_2, httpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

            Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("Meow".getBytes()), false);
            http2DataFrame.stream(http2HeadersFrame.stream());
            embeddedChannel.writeInbound(http2DataFrame);
            embeddedChannel.flushInbound();
            DefaultHttp2TranslatedHttpContent httpContent = embeddedChannel.readInbound();

            assertEquals(4, httpContent.streamId());
            assertEquals("Meow", new String(ByteBufUtil.getBytes(httpContent.content())));

            http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
            http2DataFrame.stream(http2HeadersFrame.stream());
            embeddedChannel.writeInbound(http2DataFrame);
            embeddedChannel.flushInbound();
            DefaultHttp2TranslatedLastHttpContent lastHttpContent = embeddedChannel.readInbound();

            assertEquals(4, lastHttpContent.streamId());
            assertEquals("MeowMeow", new String(ByteBufUtil.getBytes(lastHttpContent.content())));

            httpContent.release();
            lastHttpContent.release();
        }
    }
}
