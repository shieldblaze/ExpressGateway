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

import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests that {@link RequestBodySizeLimitHandler} enforces the maximum request
 * body size and returns 413 Content Too Large per RFC 9110 Section 15.5.14.
 *
 * <p>Uses Netty's {@link EmbeddedChannel} for fast, isolated pipeline testing
 * without network I/O. The handler is configured with a small limit (100 bytes)
 * to make tests fast and deterministic.</p>
 *
 * <p>This is analogous to nginx's {@code client_max_body_size} directive. When
 * the accumulated body exceeds the limit, the handler:</p>
 * <ol>
 *   <li>Stops reading from the client (setAutoRead(false))</li>
 *   <li>Sends a 413 response with Connection: close</li>
 *   <li>Closes the channel</li>
 * </ol>
 */
class LargeBodyTest {

    /**
     * Small body limit (100 bytes) for fast test execution.
     */
    private static final long MAX_BODY_SIZE = 100L;

    /**
     * Sends a single HttpContent chunk that exceeds the body size limit.
     * The handler MUST respond with 413 and close the connection.
     */
    @Test
    void bodyExceedingLimit_returns413() {
        EmbeddedChannel channel = new EmbeddedChannel(new RequestBodySizeLimitHandler(MAX_BODY_SIZE));

        // Send the HttpRequest (headers only -- no body bytes counted)
        DefaultHttpRequest request = new DefaultHttpRequest(
                HttpVersion.HTTP_1_1, HttpMethod.POST, "/upload");
        request.headers().set(HttpHeaderNames.HOST, "localhost");
        request.headers().set(HttpHeaderNames.CONTENT_LENGTH, 200);
        channel.writeInbound(request);

        // Send body content that exceeds the 100-byte limit
        byte[] bodyBytes = new byte[200];
        HttpContent content = new DefaultHttpContent(Unpooled.wrappedBuffer(bodyBytes));
        channel.writeInbound(content);

        // The handler should have written a 413 response to the outbound
        Object outbound = channel.readOutbound();
        assertNotNull(outbound, "Expected a 413 response to be written");
        assertTrue(outbound instanceof FullHttpResponse,
                "Expected FullHttpResponse but got: " + outbound.getClass().getSimpleName());

        FullHttpResponse response = (FullHttpResponse) outbound;
        assertEquals(HttpResponseStatus.REQUEST_ENTITY_TOO_LARGE, response.status(),
                "Response status must be 413 Content Too Large");
        assertEquals("close", response.headers().get(HttpHeaderNames.CONNECTION),
                "Response must include Connection: close");

        // The body should contain the rejection message
        String body = response.content().toString(StandardCharsets.UTF_8);
        assertTrue(body.contains("too large"), "Response body should explain the rejection");

        response.release();
        // Channel should be closed after 413
        assertFalse(channel.isOpen(), "Channel must be closed after 413 response");
    }

    /**
     * Sends body content within the limit. The handler MUST pass the request
     * and content through to the next handler in the pipeline without
     * generating a 413 response.
     */
    @Test
    void bodyWithinLimit_passesThrough() {
        EmbeddedChannel channel = new EmbeddedChannel(new RequestBodySizeLimitHandler(MAX_BODY_SIZE));

        // Send the HttpRequest
        DefaultHttpRequest request = new DefaultHttpRequest(
                HttpVersion.HTTP_1_1, HttpMethod.POST, "/upload");
        request.headers().set(HttpHeaderNames.HOST, "localhost");
        request.headers().set(HttpHeaderNames.CONTENT_LENGTH, 50);
        channel.writeInbound(request);

        // Send body content within the 100-byte limit
        byte[] bodyBytes = new byte[50];
        DefaultLastHttpContent lastContent = new DefaultLastHttpContent(Unpooled.wrappedBuffer(bodyBytes));
        channel.writeInbound(lastContent);

        // The request and content should have been passed through (inbound)
        Object readRequest = channel.readInbound();
        assertNotNull(readRequest, "Request should pass through the handler");

        Object readContent = channel.readInbound();
        assertNotNull(readContent, "Content should pass through the handler");

        // No 413 response should have been generated
        Object outbound = channel.readOutbound();
        assertNull(outbound, "No outbound response should be generated for valid requests");

        channel.finishAndReleaseAll();
    }

    /**
     * Sends multiple chunks where the cumulative size exceeds the limit on
     * the second chunk. The first chunk should pass through; the second
     * should trigger a 413.
     */
    @Test
    void cumulativeChunksExceedingLimit_returns413() {
        EmbeddedChannel channel = new EmbeddedChannel(new RequestBodySizeLimitHandler(MAX_BODY_SIZE));

        // Send the HttpRequest
        DefaultHttpRequest request = new DefaultHttpRequest(
                HttpVersion.HTTP_1_1, HttpMethod.POST, "/upload");
        request.headers().set(HttpHeaderNames.HOST, "localhost");
        request.headers().set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
        channel.writeInbound(request);

        // First chunk: 60 bytes (within the 100-byte limit)
        byte[] chunk1 = new byte[60];
        channel.writeInbound(new DefaultHttpContent(Unpooled.wrappedBuffer(chunk1)));

        // First chunk should pass through
        channel.readInbound(); // request
        Object firstChunk = channel.readInbound();
        assertNotNull(firstChunk, "First chunk within limit should pass through");

        // Second chunk: 60 bytes (cumulative = 120 bytes, exceeds limit)
        byte[] chunk2 = new byte[60];
        channel.writeInbound(new DefaultHttpContent(Unpooled.wrappedBuffer(chunk2)));

        // 413 response should now be in outbound
        Object outbound = channel.readOutbound();
        assertNotNull(outbound, "Expected a 413 response after cumulative limit exceeded");
        assertTrue(outbound instanceof FullHttpResponse);

        FullHttpResponse response = (FullHttpResponse) outbound;
        assertEquals(HttpResponseStatus.REQUEST_ENTITY_TOO_LARGE, response.status());
        response.release();

        assertFalse(channel.isOpen(), "Channel must be closed after 413 response");
    }

    /**
     * Verifies that the handler resets state between requests on a keep-alive
     * connection. A small first request followed by a small second request
     * should both pass through.
     */
    @Test
    void newRequestResetsAccumulator() {
        EmbeddedChannel channel = new EmbeddedChannel(new RequestBodySizeLimitHandler(MAX_BODY_SIZE));

        // First request with 80-byte body (within limit)
        DefaultHttpRequest request1 = new DefaultHttpRequest(
                HttpVersion.HTTP_1_1, HttpMethod.POST, "/first");
        request1.headers().set(HttpHeaderNames.HOST, "localhost");
        request1.headers().set(HttpHeaderNames.CONTENT_LENGTH, 80);
        channel.writeInbound(request1);
        channel.writeInbound(new DefaultLastHttpContent(Unpooled.wrappedBuffer(new byte[80])));

        // Drain inbound
        channel.readInbound();
        channel.readInbound();

        // Second request with 80-byte body (within limit -- accumulator must have reset)
        DefaultHttpRequest request2 = new DefaultHttpRequest(
                HttpVersion.HTTP_1_1, HttpMethod.POST, "/second");
        request2.headers().set(HttpHeaderNames.HOST, "localhost");
        request2.headers().set(HttpHeaderNames.CONTENT_LENGTH, 80);
        channel.writeInbound(request2);
        channel.writeInbound(new DefaultLastHttpContent(Unpooled.wrappedBuffer(new byte[80])));

        // Both requests should pass through without triggering 413
        Object outbound = channel.readOutbound();
        assertNull(outbound, "No 413 should be generated when each request is within limit");

        channel.finishAndReleaseAll();
    }
}
