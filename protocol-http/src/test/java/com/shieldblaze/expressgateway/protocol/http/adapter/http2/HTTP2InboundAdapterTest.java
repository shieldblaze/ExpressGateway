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

import com.shieldblaze.expressgateway.protocol.http.NonceWrapped;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
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
        NonceWrapped<FullHttpRequest> fullHttpRequest = embeddedChannel.readInbound();

        assertEquals("GET", fullHttpRequest.get().method().name());
        assertEquals("/", fullHttpRequest.get().uri());
        assertEquals("localhost", fullHttpRequest.get().headers().get("host"));

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(fullHttpRequest.nonce(), new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK,
                Unpooled.wrappedBuffer("Meow".getBytes())));
        httpResponse.get().headers().set("x-meow-key", "MeowXD");

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

        fullHttpRequest.get().release();
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
        NonceWrapped<FullHttpRequest> fullHttpRequest = embeddedChannel.readInbound();

        assertEquals("POST", fullHttpRequest.get().method().name());
        assertEquals("/meow", fullHttpRequest.get().uri());
        assertEquals("www.shieldblaze.com", fullHttpRequest.get().headers().get("host"));

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(fullHttpRequest.nonce(),
                new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, Unpooled.EMPTY_BUFFER));
        httpResponse.get().headers().set("x-meow-key", "MeowXD");

        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();
        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();

        assertTrue(responseHeadersFrame.isEndStream());
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals("MeowXD", responseHeadersFrame.headers().get("x-meow-key").toString());

        fullHttpRequest.get().release();
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

        NonceWrapped<HttpRequest> httpRequest = embeddedChannel.readInbound();

        assertEquals("POST", httpRequest.get().method().name());
        assertEquals("/meow", httpRequest.get().uri());
        assertEquals("www.shieldblaze.com", httpRequest.get().headers().get("host"));

        Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        NonceWrapped<LastHttpContent> lastHttpContent = embeddedChannel.readInbound();

        assertEquals("MeowMeow", new String(ByteBufUtil.getBytes(lastHttpContent.get().content())));
        lastHttpContent.get().release();
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
        NonceWrapped<HttpRequest> httpRequest = embeddedChannel.readInbound();

        assertEquals("POST", httpRequest.get().method().name());
        assertEquals("/meow", httpRequest.get().uri());
        assertEquals("www.shieldblaze.com", httpRequest.get().headers().get("host"));

        Http2DataFrame http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("Meow".getBytes()), false);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        NonceWrapped<HttpContent> httpContent = embeddedChannel.readInbound();

        assertEquals("Meow", new String(ByteBufUtil.getBytes(httpContent.get().content())));

        http2DataFrame = new DefaultHttp2DataFrame(Unpooled.wrappedBuffer("MeowMeow".getBytes()), true);
        http2DataFrame.stream(http2HeadersFrame.stream());
        embeddedChannel.writeInbound(http2DataFrame);
        embeddedChannel.flushInbound();
        NonceWrapped<LastHttpContent> lastHttpContent = embeddedChannel.readInbound();

        assertEquals("MeowMeow", new String(ByteBufUtil.getBytes(lastHttpContent.get().content())));

        httpContent.get().release();
        lastHttpContent.get().release();
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
        NonceWrapped<FullHttpRequest> fullHttpRequest = embeddedChannel.readInbound();

        final int bytesCount = 1024 * 1000;

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(fullHttpRequest.nonce(), new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK));
        httpResponse.get().headers().set(HttpHeaderNames.CONTENT_LENGTH, bytesCount);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());
        assertEquals(String.valueOf(bytesCount), responseHeadersFrame.headers().get(HttpHeaderNames.CONTENT_LENGTH));

        Random random = new Random();
        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            random.nextBytes(bytes);

            NonceWrapped<HttpContent> httpContent;
            if (i == bytesCount) {
                httpContent = new NonceWrapped<>(httpResponse.nonce(), new DefaultLastHttpContent(Unpooled.wrappedBuffer(bytes)));
            } else {
                httpContent = new NonceWrapped<>(httpResponse.nonce(), new DefaultHttpContent(Unpooled.wrappedBuffer(bytes)));
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

        fullHttpRequest.get().release();
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
        NonceWrapped<FullHttpRequest> fullHttpRequest = embeddedChannel.readInbound();

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(fullHttpRequest.nonce(), new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK));
        httpResponse.get().headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        Http2HeadersFrame responseHeadersFrame = embeddedChannel.readOutbound();
        assertEquals("200", responseHeadersFrame.headers().status().toString());

        final Random random = new Random();
        final int bytesCount = 1024 * 1000;
        for (int i = 1; i <= bytesCount; i++) {
            byte[] bytes = new byte[1];
            random.nextBytes(bytes);

            NonceWrapped<HttpContent> httpContent;
            if (i == bytesCount) {
                httpContent = new NonceWrapped<>(httpResponse.nonce(), new DefaultLastHttpContent(Unpooled.wrappedBuffer(bytes)));
            } else {
                httpContent = new NonceWrapped<>(httpResponse.nonce(), new DefaultHttpContent(Unpooled.wrappedBuffer(bytes)));
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
