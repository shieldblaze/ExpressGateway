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
package com.shieldblaze.expressgateway.protocol.http.translator;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http3.DefaultHttp3DataFrame;
import io.netty.handler.codec.http3.DefaultHttp3Headers;
import io.netty.handler.codec.http3.DefaultHttp3HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for the sealed {@link ProtocolTranslator} hierarchy and all 6 translator
 * implementations. Validates header transformation, body forwarding, trailer
 * handling, and end-of-stream semantics for each protocol pair.
 */
class ProtocolTranslatorTest {

    // ═══════════════════════════════════════════════════════════════════════
    // Factory method
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class Factory {

        @Test
        void forPairReturnsCorrectTranslator() {
            assertInstanceOf(H1ToH2Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_2));
            assertInstanceOf(H1ToH3Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_3));
            assertInstanceOf(H2ToH1Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_2, ProtocolVersion.HTTP_1_1));
            assertInstanceOf(H2ToH3Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_2, ProtocolVersion.HTTP_3));
            assertInstanceOf(H3ToH1Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_3, ProtocolVersion.HTTP_1_1));
            assertInstanceOf(H3ToH2Translator.class, ProtocolTranslator.forPair(ProtocolVersion.HTTP_3, ProtocolVersion.HTTP_2));
        }

        @Test
        void sameProtocolThrows() {
            assertThrows(IllegalArgumentException.class, () -> ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_1_1));
            assertThrows(IllegalArgumentException.class, () -> ProtocolTranslator.forPair(ProtocolVersion.HTTP_2, ProtocolVersion.HTTP_2));
            assertThrows(IllegalArgumentException.class, () -> ProtocolTranslator.forPair(ProtocolVersion.HTTP_3, ProtocolVersion.HTTP_3));
        }

        @Test
        void exhaustiveSwitchOnSealed() {
            // Verify pattern matching works on sealed type and returns correct protocol pair
            ProtocolTranslator t = ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_2);
            String result = switch (t) {
                case H1ToH2Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
                case H1ToH3Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
                case H2ToH1Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
                case H2ToH3Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
                case H3ToH1Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
                case H3ToH2Translator h -> h.sourceProtocol() + "→" + h.targetProtocol();
            };
            assertEquals("HTTP_1_1→HTTP_2", result);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H1 → H2 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H1ToH2 {

        private final H1ToH2Translator translator = (H1ToH2Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_2);

        @Test
        void translateRequestHeaders() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/api/data");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.ACCEPT, "application/json");

            Http2HeadersFrame frame = translator.translateRequest(request, true);

            assertEquals("GET", frame.headers().method().toString());
            assertEquals("/api/data", frame.headers().path().toString());
            assertEquals("https", frame.headers().scheme().toString());
            assertEquals("example.com", frame.headers().authority().toString());
            assertFalse(frame.isEndStream()); // Not a FullHttpRequest
        }

        @Test
        void translateFullRequestWithEmptyBody() {
            DefaultFullHttpRequest request = new DefaultFullHttpRequest(
                    HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");

            Http2HeadersFrame frame = translator.translateRequest(request, true);

            assertTrue(frame.isEndStream()); // Empty FullHttpRequest → endStream
        }

        @Test
        void translateBodyChunk() {
            ByteBuf body = Unpooled.copiedBuffer("hello world", StandardCharsets.UTF_8);
            HttpContent content = new DefaultHttpContent(body);

            Http2DataFrame dataFrame = translator.translateBody(content, false);

            assertEquals("hello world", dataFrame.content().toString(StandardCharsets.UTF_8));
            assertFalse(dataFrame.isEndStream());

            dataFrame.release();
            content.release();
        }

        @Test
        void translateBodyWithEndStream() {
            ByteBuf body = Unpooled.copiedBuffer("final chunk", StandardCharsets.UTF_8);
            HttpContent content = new DefaultHttpContent(body);

            Http2DataFrame dataFrame = translator.translateBody(content, true);

            assertTrue(dataFrame.isEndStream());

            dataFrame.release();
            content.release();
        }

        @Test
        void translateTrailers() {
            DefaultLastHttpContent lastContent = new DefaultLastHttpContent();
            lastContent.trailingHeaders().set("x-checksum", "abc123");
            lastContent.trailingHeaders().set("grpc-status", "0");

            Http2HeadersFrame trailerFrame = translator.translateTrailers(lastContent);

            assertTrue(trailerFrame.isEndStream());
            assertEquals("abc123", trailerFrame.headers().get("x-checksum").toString());
            assertEquals("0", trailerFrame.headers().get("grpc-status").toString());
        }

        @Test
        void translateResponse() {
            HttpResponse response = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");

            Http2HeadersFrame frame = translator.translateResponse(response);

            assertEquals("200", frame.headers().status().toString());
            assertFalse(frame.isEndStream()); // Regular response, not FullHttpResponse
        }

        @Test
        void translateFullResponseWithEmptyBody() {
            DefaultFullHttpResponse response = new DefaultFullHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.NO_CONTENT);

            Http2HeadersFrame frame = translator.translateResponse(response);

            assertTrue(frame.isEndStream()); // Empty body → endStream
        }

        @Test
        void viaHeaderInjected() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "x.com");

            Http2HeadersFrame frame = translator.translateRequest(request, false);

            assertEquals("1.1 expressgateway", frame.headers().get("via").toString());
        }

        @Test
        void sourceAndTargetProtocol() {
            assertEquals(ProtocolVersion.HTTP_1_1, translator.sourceProtocol());
            assertEquals(ProtocolVersion.HTTP_2, translator.targetProtocol());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H1 → H3 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H1ToH3 {

        private final H1ToH3Translator translator = (H1ToH3Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_1_1, ProtocolVersion.HTTP_3);

        @Test
        void translateRequestHeaders() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/submit");
            request.headers().set(HttpHeaderNames.HOST, "cdn.example.com");
            request.headers().set(HttpHeaderNames.CONTENT_TYPE, "application/octet-stream");

            Http3HeadersFrame frame = translator.translateRequest(request, true);

            assertEquals("POST", frame.headers().method().toString());
            assertEquals("/submit", frame.headers().path().toString());
            assertEquals("https", frame.headers().scheme().toString());
            assertEquals("cdn.example.com", frame.headers().authority().toString());
        }

        @Test
        void translateBody() {
            ByteBuf body = Unpooled.copiedBuffer("body data", StandardCharsets.UTF_8);
            HttpContent content = new DefaultHttpContent(body);

            Http3DataFrame dataFrame = translator.translateBody(content);

            assertEquals("body data", dataFrame.content().toString(StandardCharsets.UTF_8));

            dataFrame.release();
            content.release();
        }

        @Test
        void translateTrailers() {
            DefaultLastHttpContent lastContent = new DefaultLastHttpContent();
            lastContent.trailingHeaders().set("x-hash", "sha256");

            Http3HeadersFrame trailerFrame = translator.translateTrailers(lastContent);

            assertEquals("sha256", trailerFrame.headers().get("x-hash").toString());
        }

        @Test
        void isEmptyBody() {
            DefaultFullHttpRequest emptyBody = new DefaultFullHttpRequest(
                    HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            assertTrue(translator.isEmptyBody(emptyBody));

            ByteBuf content = Unpooled.copiedBuffer("data", StandardCharsets.UTF_8);
            DefaultFullHttpRequest withBody = new DefaultFullHttpRequest(
                    HttpVersion.HTTP_1_1, HttpMethod.POST, "/", content);
            assertFalse(translator.isEmptyBody(withBody));
            withBody.release();

            // Non-full request (streaming) is never "empty" at this stage
            HttpRequest streaming = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/");
            assertFalse(translator.isEmptyBody(streaming));
        }

        @Test
        void viaHeaderInjected() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "x.com");

            Http3HeadersFrame frame = translator.translateRequest(request, false);

            assertEquals("1.1 expressgateway", frame.headers().get("via").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H2 → H1 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H2ToH1 {

        private final H2ToH1Translator translator = (H2ToH1Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_2, ProtocolVersion.HTTP_1_1);

        @Test
        void translateRequestHeaders() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/api/resource");
            h2.scheme("https");
            h2.authority("api.example.com");
            h2.set("accept", "application/json");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, false);
            HttpRequest request = translator.translateRequest(frame);

            assertEquals(HttpMethod.GET, request.method());
            assertEquals("/api/resource", request.uri());
            assertEquals(HttpVersion.HTTP_1_1, request.protocolVersion());
            assertEquals("api.example.com", request.headers().get(HttpHeaderNames.HOST));
            assertEquals("application/json", request.headers().get("accept"));
        }

        @Test
        void translateResponse() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.status("200");
            h2.set("content-type", "application/json");
            h2.set("x-request-id", "req-42");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, false);
            HttpResponse response = translator.translateResponse(frame);

            assertEquals(HttpResponseStatus.OK, response.status());
            assertEquals(HttpVersion.HTTP_1_1, response.protocolVersion());
            assertEquals("application/json", response.headers().get("content-type"));
            assertEquals("req-42", response.headers().get("x-request-id"));
        }

        @Test
        void translateBodyNotEndStream() {
            ByteBuf body = Unpooled.copiedBuffer("chunk", StandardCharsets.UTF_8);
            Http2DataFrame dataFrame = new DefaultHttp2DataFrame(body, false);

            HttpContent content = translator.translateBody(dataFrame);

            assertInstanceOf(DefaultHttpContent.class, content);
            assertFalse(content instanceof LastHttpContent);
            assertEquals("chunk", content.content().toString(StandardCharsets.UTF_8));

            content.release();
        }

        @Test
        void translateBodyEndStream() {
            ByteBuf body = Unpooled.copiedBuffer("final", StandardCharsets.UTF_8);
            Http2DataFrame dataFrame = new DefaultHttp2DataFrame(body, true);

            HttpContent content = translator.translateBody(dataFrame);

            assertInstanceOf(LastHttpContent.class, content);
            assertEquals("final", content.content().toString(StandardCharsets.UTF_8));

            content.release();
        }

        @Test
        void translateTrailers() {
            Http2Headers trailers = new DefaultHttp2Headers();
            trailers.set("grpc-status", "0");
            trailers.set("grpc-message", "OK");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(trailers, true);
            LastHttpContent lastContent = translator.translateTrailers(frame);

            assertEquals("0", lastContent.trailingHeaders().get("grpc-status"));
            assertEquals("OK", lastContent.trailingHeaders().get("grpc-message"));
        }

        @Test
        void endOfMessage() {
            LastHttpContent eom = translator.endOfMessage();
            assertEquals(LastHttpContent.EMPTY_LAST_CONTENT, eom);
        }

        @Test
        void viaHeaderInjected() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");
            h2.authority("x.com");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, true);
            HttpRequest request = translator.translateRequest(frame);

            assertEquals("2.0 expressgateway", request.headers().get("via"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H2 → H3 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H2ToH3 {

        private final H2ToH3Translator translator = (H2ToH3Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_2, ProtocolVersion.HTTP_3);

        @Test
        void translateRequest() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("PUT");
            h2.path("/resource");
            h2.scheme("https");
            h2.authority("example.com");
            h2.set("content-type", "application/json");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, false);
            Http3HeadersFrame h3Frame = translator.translateRequest(frame);

            assertEquals("PUT", h3Frame.headers().method().toString());
            assertEquals("/resource", h3Frame.headers().path().toString());
            assertEquals("https", h3Frame.headers().scheme().toString());
            assertEquals("example.com", h3Frame.headers().authority().toString());
        }

        @Test
        void translateResponse() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.status("404");
            h2.set("content-type", "text/plain");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, false);
            Http3HeadersFrame h3Frame = translator.translateResponse(frame);

            assertEquals("404", h3Frame.headers().status().toString());
        }

        @Test
        void translateBody() {
            ByteBuf body = Unpooled.copiedBuffer("data", StandardCharsets.UTF_8);
            Http2DataFrame dataFrame = new DefaultHttp2DataFrame(body, false);

            Http3DataFrame h3Data = translator.translateBody(dataFrame);

            assertEquals("data", h3Data.content().toString(StandardCharsets.UTF_8));

            h3Data.release();
        }

        @Test
        void translateTrailers() {
            Http2Headers trailers = new DefaultHttp2Headers();
            trailers.set("grpc-status", "0");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(trailers, true);
            Http3HeadersFrame h3Trailers = translator.translateTrailers(frame);

            assertEquals("0", h3Trailers.headers().get("grpc-status").toString());
        }

        @Test
        void isEndStreamOnHeaders() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");

            assertFalse(translator.isEndStream(new DefaultHttp2HeadersFrame(h2, false)));
            assertTrue(translator.isEndStream(new DefaultHttp2HeadersFrame(h2, true)));
        }

        @Test
        void isEndStreamOnData() {
            ByteBuf body = Unpooled.copiedBuffer("x", StandardCharsets.UTF_8);
            ByteBuf body2 = Unpooled.copiedBuffer("y", StandardCharsets.UTF_8);

            assertFalse(translator.isEndStream(new DefaultHttp2DataFrame(body, false)));
            assertTrue(translator.isEndStream(new DefaultHttp2DataFrame(body2, true)));

            body.release();
            body2.release();
        }

        @Test
        void viaHeaderInjected() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, false);
            Http3HeadersFrame h3 = translator.translateRequest(frame);

            assertEquals("2.0 expressgateway", h3.headers().get("via").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H3 → H1 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H3ToH1 {

        private final H3ToH1Translator translator = (H3ToH1Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_3, ProtocolVersion.HTTP_1_1);

        @Test
        void translateRequest() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("DELETE");
            h3.path("/resource/42");
            h3.scheme("https");
            h3.authority("api.example.com");
            h3.set("authorization", "Bearer tok");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            HttpRequest request = translator.translateRequest(frame);

            assertEquals(HttpMethod.DELETE, request.method());
            assertEquals("/resource/42", request.uri());
            assertEquals("api.example.com", request.headers().get(HttpHeaderNames.HOST));
            assertEquals("Bearer tok", request.headers().get("authorization"));
        }

        @Test
        void translateResponse() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.status("201");
            h3.set("location", "/new-resource");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            HttpResponse response = translator.translateResponse(frame);

            assertEquals(HttpResponseStatus.CREATED, response.status());
            assertEquals("/new-resource", response.headers().get("location"));
        }

        @Test
        void translateBody() {
            ByteBuf body = Unpooled.copiedBuffer("stream data", StandardCharsets.UTF_8);
            Http3DataFrame dataFrame = new DefaultHttp3DataFrame(body);

            HttpContent content = translator.translateBody(dataFrame);

            assertEquals("stream data", content.content().toString(StandardCharsets.UTF_8));

            content.release();
        }

        @Test
        void translateTrailers() {
            Http3Headers trailers = new DefaultHttp3Headers();
            trailers.set("grpc-status", "2");
            trailers.set("grpc-message", "UNKNOWN");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(trailers);
            LastHttpContent lastContent = translator.translateTrailers(frame);

            assertEquals("2", lastContent.trailingHeaders().get("grpc-status"));
            assertEquals("UNKNOWN", lastContent.trailingHeaders().get("grpc-message"));
        }

        @Test
        void endOfMessage() {
            LastHttpContent eom = translator.endOfMessage();
            assertEquals(LastHttpContent.EMPTY_LAST_CONTENT, eom);
        }

        @Test
        void viaHeaderInjected() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");
            h3.authority("x.com");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            HttpRequest request = translator.translateRequest(frame);

            assertEquals("3.0 expressgateway", request.headers().get("via"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H3 → H2 Translator
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H3ToH2 {

        private final H3ToH2Translator translator = (H3ToH2Translator)
                ProtocolTranslator.forPair(ProtocolVersion.HTTP_3, ProtocolVersion.HTTP_2);

        @Test
        void translateRequest() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("OPTIONS");
            h3.path("*");
            h3.scheme("https");
            h3.authority("example.com");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            Http2HeadersFrame h2Frame = translator.translateRequest(frame, false);

            assertEquals("OPTIONS", h2Frame.headers().method().toString());
            assertEquals("*", h2Frame.headers().path().toString());
            assertFalse(h2Frame.isEndStream());
        }

        @Test
        void translateRequestWithEndStream() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            Http2HeadersFrame h2Frame = translator.translateRequest(frame, true);

            assertTrue(h2Frame.isEndStream());
        }

        @Test
        void translateResponse() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.status("200");
            h3.set("content-type", "application/json");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            Http2HeadersFrame h2Frame = translator.translateResponse(frame, false);

            assertEquals("200", h2Frame.headers().status().toString());
            assertFalse(h2Frame.isEndStream());
        }

        @Test
        void translateBody() {
            ByteBuf body = Unpooled.copiedBuffer("payload", StandardCharsets.UTF_8);
            Http3DataFrame dataFrame = new DefaultHttp3DataFrame(body);

            Http2DataFrame h2Data = translator.translateBody(dataFrame, false);

            assertEquals("payload", h2Data.content().toString(StandardCharsets.UTF_8));
            assertFalse(h2Data.isEndStream());

            h2Data.release();
        }

        @Test
        void translateBodyEndStream() {
            ByteBuf body = Unpooled.copiedBuffer("last", StandardCharsets.UTF_8);
            Http3DataFrame dataFrame = new DefaultHttp3DataFrame(body);

            Http2DataFrame h2Data = translator.translateBody(dataFrame, true);

            assertTrue(h2Data.isEndStream());

            h2Data.release();
        }

        @Test
        void translateTrailers() {
            Http3Headers trailers = new DefaultHttp3Headers();
            trailers.set("x-digest", "sha256=abc");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(trailers);
            Http2HeadersFrame h2Trailers = translator.translateTrailers(frame);

            assertTrue(h2Trailers.isEndStream());
            assertEquals("sha256=abc", h2Trailers.headers().get("x-digest").toString());
        }

        @Test
        void endOfStream() {
            Http2DataFrame eos = translator.endOfStream();
            assertTrue(eos.isEndStream());
            assertEquals(0, eos.content().readableBytes());

            eos.release();
        }

        @Test
        void viaHeaderInjected() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            Http2HeadersFrame h2 = translator.translateRequest(frame, false);

            assertEquals("3.0 expressgateway", h2.headers().get("via").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Large body handling
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class LargeBody {

        @Test
        void h1ToH2LargeBody() {
            H1ToH2Translator translator = H1ToH2Translator.INSTANCE;
            byte[] largePayload = new byte[1024 * 1024]; // 1 MB
            java.util.Arrays.fill(largePayload, (byte) 'A');
            ByteBuf body = Unpooled.wrappedBuffer(largePayload);
            HttpContent content = new DefaultHttpContent(body);

            Http2DataFrame frame = translator.translateBody(content, false);

            assertEquals(1024 * 1024, frame.content().readableBytes());
            assertFalse(frame.isEndStream());

            frame.release();
            content.release();
        }

        @Test
        void h2ToH1LargeBody() {
            H2ToH1Translator translator = H2ToH1Translator.INSTANCE;
            byte[] largePayload = new byte[1024 * 1024];
            java.util.Arrays.fill(largePayload, (byte) 'B');
            ByteBuf body = Unpooled.wrappedBuffer(largePayload);
            Http2DataFrame dataFrame = new DefaultHttp2DataFrame(body, false);

            HttpContent content = translator.translateBody(dataFrame);

            assertEquals(1024 * 1024, content.content().readableBytes());

            content.release();
        }

        @Test
        void h2ToH3LargeBody() {
            H2ToH3Translator translator = H2ToH3Translator.INSTANCE;
            byte[] largePayload = new byte[512 * 1024]; // 512 KB
            ByteBuf body = Unpooled.wrappedBuffer(largePayload);
            Http2DataFrame dataFrame = new DefaultHttp2DataFrame(body, true);

            Http3DataFrame h3Data = translator.translateBody(dataFrame);

            assertEquals(512 * 1024, h3Data.content().readableBytes());

            h3Data.release();
        }

        @Test
        void h3ToH1LargeBody() {
            H3ToH1Translator translator = H3ToH1Translator.INSTANCE;
            byte[] largePayload = new byte[2 * 1024 * 1024]; // 2 MB
            ByteBuf body = Unpooled.wrappedBuffer(largePayload);
            Http3DataFrame dataFrame = new DefaultHttp3DataFrame(body);

            HttpContent content = translator.translateBody(dataFrame);

            assertEquals(2 * 1024 * 1024, content.content().readableBytes());

            content.release();
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Error propagation
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class ErrorPropagation {

        @Test
        void h2ToH1ErrorResponse() {
            H2ToH1Translator translator = H2ToH1Translator.INSTANCE;

            Http2Headers h2 = new DefaultHttp2Headers();
            h2.status("502");
            h2.set("content-length", "0");

            Http2HeadersFrame frame = new DefaultHttp2HeadersFrame(h2, true);
            HttpResponse response = translator.translateResponse(frame);

            assertEquals(HttpResponseStatus.BAD_GATEWAY, response.status());
        }

        @Test
        void h3ToH1ErrorResponse() {
            H3ToH1Translator translator = H3ToH1Translator.INSTANCE;

            Http3Headers h3 = new DefaultHttp3Headers();
            h3.status("503");

            Http3HeadersFrame frame = new DefaultHttp3HeadersFrame(h3);
            HttpResponse response = translator.translateResponse(frame);

            assertEquals(HttpResponseStatus.SERVICE_UNAVAILABLE, response.status());
        }

        @Test
        void h1ToH2ErrorResponse() {
            H1ToH2Translator translator = H1ToH2Translator.INSTANCE;

            HttpResponse response = new DefaultHttpResponse(
                    HttpVersion.HTTP_1_1, HttpResponseStatus.GATEWAY_TIMEOUT);

            Http2HeadersFrame frame = translator.translateResponse(response);

            assertEquals("504", frame.headers().status().toString());
        }
    }
}
