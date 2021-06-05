/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.CustomFullHttpRequest;
import io.netty.handler.codec.http.CustomFullHttpResponse;
import io.netty.handler.codec.http.CustomHttpContent;
import io.netty.handler.codec.http.CustomHttpRequest;
import io.netty.handler.codec.http.CustomHttpResponse;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
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
        CustomFullHttpRequest fullHttpRequest = embeddedChannel.readInbound();
        assertEquals(HttpFrame.Protocol.H2, fullHttpRequest.protocol());

        assertEquals("GET", fullHttpRequest.method().name());
        assertEquals("/", fullHttpRequest.uri());
        assertEquals("localhost", fullHttpRequest.headers().get("host"));

        HttpResponse httpResponse = new CustomFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK,
                Unpooled.wrappedBuffer("Meow".getBytes()), HttpFrame.Protocol.H2, fullHttpRequest.id());
        httpResponse.headers().set("x-meow-key", "MeowXD");

        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();
        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();

        assertFalse(responseHeadersFrame.isEndStream());
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals("MeowXD", responseHeadersFrame.headers().get("x-meow-key").toString());

        Http2DataFrame responseDataFrame = embeddedChannel.readOutbound();
        assertTrue(responseDataFrame.isEndStream());
        assertEquals("Meow", new String(ByteBufUtil.getBytes(responseDataFrame.content())));
        assertEquals(2, responseDataFrame.stream().id());

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
        HttpFrame httpFrame = (HttpFrame) fullHttpRequest;
        assertEquals(HttpFrame.Protocol.H2, httpFrame.protocol());

        assertEquals("POST", fullHttpRequest.method().name());
        assertEquals("/meow", fullHttpRequest.uri());
        assertEquals("www.shieldblaze.com", fullHttpRequest.headers().get("host"));

        HttpResponse httpResponse = new CustomFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.EMPTY_BUFFER,
                HttpFrame.Protocol.H2, httpFrame.id());
        httpResponse.headers().set("x-meow-key", "MeowXD");

        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();
        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();

        assertTrue(responseHeadersFrame.isEndStream());
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals("MeowXD", responseHeadersFrame.headers().get("x-meow-key").toString());

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
        HttpFrame httpFrame = (HttpFrame) httpRequest;
        assertEquals(HttpFrame.Protocol.H2, httpFrame.protocol());

        assertEquals("POST", httpRequest.method().name());
        assertEquals("/meow", httpRequest.uri());
        assertEquals("www.shieldblaze.com", httpRequest.headers().get("host"));

        Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        CustomLastHttpContent lastHttpContent = embeddedChannel.readInbound();

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
        HttpFrame httpFrame = (HttpFrame) httpRequest;
        assertEquals(HttpFrame.Protocol.H2, httpFrame.protocol());

        assertEquals("POST", httpRequest.method().name());
        assertEquals("/meow", httpRequest.uri());
        assertEquals("www.shieldblaze.com", httpRequest.headers().get("host"));

        Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("Meow".getBytes()), false);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        CustomHttpContent httpContent = embeddedChannel.readInbound();

        assertEquals("Meow", new String(ByteBufUtil.getBytes(httpContent.content())));

        http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        CustomLastHttpContent lastHttpContent = embeddedChannel.readInbound();

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
        CustomFullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

        final int bytesCount = 1024 * 1000;

        HttpResponse httpResponse = new CustomHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, HttpFrame.Protocol.H2, fullHttpRequest.id());
        httpResponse.headers().set(HttpHeaderNames.CONTENT_LENGTH, bytesCount);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals(String.valueOf(bytesCount), responseHeadersFrame.headers().get(HttpHeaderNames.CONTENT_LENGTH));

        long id = ((HttpFrame) httpResponse).id();

        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            HttpContent httpContent;
            if (i == bytesCount) {
                httpContent = new CustomLastHttpContent(Unpooled.wrappedBuffer(bytes), HttpFrame.Protocol.H2, id);
            } else {
                httpContent = new CustomHttpContent(Unpooled.wrappedBuffer(bytes), HttpFrame.Protocol.H2, id);
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
        CustomHttpRequest fullHttpRequest = embeddedChannel.readInbound();

        HttpResponse httpResponse = new CustomHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, HttpFrame.Protocol.H2, fullHttpRequest.id());
        httpResponse.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());

        long id = ((HttpFrame) httpResponse).id();

        final Random random = new Random();
        final int bytesCount = 1024 * 1000;
        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            random.nextBytes(bytes);

            HttpContent httpContent;
            if (i == bytesCount) {
                httpContent = new CustomLastHttpContent(Unpooled.wrappedBuffer(bytes), HttpFrame.Protocol.H2, id);
            } else {
                httpContent = new CustomHttpContent(Unpooled.wrappedBuffer(bytes), HttpFrame.Protocol.H2, id);
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
    }
}
