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
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTP2InboundAdapterTest {

    EmbeddedChannel embeddedChannel;
    HTTP2InboundAdapter http2InboundAdapter;

    @BeforeEach
    void setupEmbeddedChannel() {
        http2InboundAdapter = new HTTP2InboundAdapter();
        embeddedChannel = new EmbeddedChannel(http2InboundAdapter);
    }

    @AfterEach
    void shutdownEmbeddedChannel() {
        embeddedChannel.close();
    }

    @Test
    void simpleGETRequestAndFullResponseTest() {
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
        assertEquals("2", fullHttpRequest.headers().get("x-http2-stream-id"));
        assertTrue(fullHttpRequest.headers().contains(Headers.STREAM_HASH));
        assertEquals(Headers.Values.HTTP_2, fullHttpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

        HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.wrappedBuffer("Meow".getBytes()));
        httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), "2");
        httpResponse.headers().set(Headers.STREAM_HASH, fullHttpRequest.headers().get(Headers.STREAM_HASH));
        httpResponse.headers().set("x-meow-key", "MeowXD");

        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();
        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();

        assertFalse(responseHeadersFrame.isEndStream());
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertFalse(responseHeadersFrame.headers().contains(Headers.STREAM_HASH));
        assertEquals("MeowXD", responseHeadersFrame.headers().get("x-meow-key").toString());

        Http2DataFrame responseDataFrame = embeddedChannel.readOutbound();
        assertTrue(responseDataFrame.isEndStream());
        assertEquals("Meow", new String(ByteBufUtil.getBytes(responseDataFrame.content())));
        assertEquals(2, responseDataFrame.stream().id());
        assertNull(http2InboundAdapter.streamHash(2));

        fullHttpRequest.release();
        responseDataFrame.release();
    }

    @Test
    void simplePOSTRequestAndFullResponseTest() {
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
        assertEquals("3", fullHttpRequest.headers().get("x-http2-stream-id"));
        assertTrue(fullHttpRequest.headers().contains(Headers.STREAM_HASH));
        assertEquals(Headers.Values.HTTP_2, fullHttpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

        HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), "3");
        httpResponse.headers().set(Headers.STREAM_HASH, fullHttpRequest.headers().get(Headers.STREAM_HASH));
        httpResponse.headers().set("x-meow-key", "MeowXD");

        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();
        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();

        assertTrue(responseHeadersFrame.isEndStream());
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertFalse(responseHeadersFrame.headers().contains(Headers.STREAM_HASH));
        assertEquals("MeowXD", responseHeadersFrame.headers().get("x-meow-key").toString());
        assertNull(http2InboundAdapter.streamHash(2));

        fullHttpRequest.release();
    }

    @Test
    void simplePOSTDataRequestTest() {
        Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), false);
        http2HeadersFrame.stream(new CustomHttp2FrameStream(5));
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
        assertEquals("5", httpRequest.headers().get("x-http2-stream-id"));
        assertTrue(httpRequest.headers().contains(Headers.STREAM_HASH));
        assertEquals(Headers.Values.HTTP_2, httpRequest.headers().get(Headers.X_FORWARDED_HTTP_VERSION));

        Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        DefaultHttp2TranslatedLastHttpContent lastHttpContent = embeddedChannel.readInbound();

        assertEquals(5, lastHttpContent.streamId());
        assertEquals("MeowMeow", new String(ByteBufUtil.getBytes(lastHttpContent.content())));

        lastHttpContent.release();
    }

    @Test
    void multiplePOSTDataRequestTest() {
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
        assertEquals("4", httpRequest.headers().get("x-http2-stream-id"));
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

    @Test
    void fullDuplexContentLengthBasedTest() {
        Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
        http2HeadersFrame.stream(new CustomHttp2FrameStream(2));
        http2HeadersFrame.headers().method("GET");
        http2HeadersFrame.headers().scheme("https");
        http2HeadersFrame.headers().path("/");
        http2HeadersFrame.headers().authority("localhost");

        embeddedChannel.writeInbound(http2HeadersFrame);
        embeddedChannel.flushInbound();
        FullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

        final int bytesCount = 1024 * 1000;

        HttpResponse httpResponse = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        httpResponse.headers().set(Headers.STREAM_HASH, fullHttpRequest.headers().get(Headers.STREAM_HASH));
        httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), "2");
        httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, bytesCount);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals(String.valueOf(bytesCount), responseHeadersFrame.headers().get(HttpHeaderNames.CONTENT_LENGTH));

        long streamHash = Long.parseLong(fullHttpRequest.headers().get(Headers.STREAM_HASH));

        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            DefaultHttp2TranslatedHttpContent httpContent;
            if (i == bytesCount) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.wrappedBuffer(bytes), streamHash);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(Unpooled.wrappedBuffer(bytes), streamHash);
            }
            embeddedChannel.writeOutbound(httpContent);
            embeddedChannel.flushOutbound();

            Http2DataFrame http2DataFrame = embeddedChannel.readOutbound();
            assertArrayEquals(bytes, ByteBufUtil.getBytes(http2DataFrame.content()));
            http2DataFrame.release();

            if (i == bytesCount) {
                assertTrue(http2DataFrame.isEndStream());
            } else {
                assertFalse(http2DataFrame.isEndStream());
            }
        }

        fullHttpRequest.release();
    }

    @Test
    void fullDuplexTransferEncodingChunkedTest() {
        Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
        http2HeadersFrame.stream(new CustomHttp2FrameStream(2));
        http2HeadersFrame.headers().method("GET");
        http2HeadersFrame.headers().scheme("https");
        http2HeadersFrame.headers().path("/");
        http2HeadersFrame.headers().authority("localhost");

        embeddedChannel.writeInbound(http2HeadersFrame);
        embeddedChannel.flushInbound();
        FullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

        HttpResponse httpResponse = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        httpResponse.headers().set(Headers.STREAM_HASH, fullHttpRequest.headers().get(Headers.STREAM_HASH));
        httpResponse.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), "2");
        httpResponse.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());

        long streamHash = Long.parseLong(fullHttpRequest.headers().get(Headers.STREAM_HASH));

        final int bytesCount = 1024 * 1000;
        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            DefaultHttp2TranslatedHttpContent httpContent;
            if (i == bytesCount) {
                httpContent = new DefaultHttp2TranslatedLastHttpContent(Unpooled.wrappedBuffer(bytes), streamHash);
            } else {
                httpContent = new DefaultHttp2TranslatedHttpContent(Unpooled.wrappedBuffer(bytes), streamHash);
            }
            embeddedChannel.writeOutbound(httpContent);
            embeddedChannel.flushOutbound();

            Http2DataFrame http2DataFrame = embeddedChannel.readOutbound();
            assertArrayEquals(bytes, ByteBufUtil.getBytes(http2DataFrame.content()));
            http2DataFrame.release();

            if (i == bytesCount) {
                assertTrue(http2DataFrame.isEndStream());
            } else {
                assertFalse(http2DataFrame.isEndStream());
            }
        }

        fullHttpRequest.release();
    }
}
