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

import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.Http2Headers;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link HopByHopHeaders} covering RFC 7230 Section 6.1 compliance.
 *
 * <p>A proxy MUST remove hop-by-hop headers before forwarding. The standard
 * hop-by-hop headers are: Connection, Keep-Alive, Proxy-Authenticate,
 * Proxy-Authorization, TE, Trailers, Transfer-Encoding, and Upgrade
 * (except for WebSocket upgrades).
 *
 * <p>Additionally, any headers named in the Connection header value must
 * also be removed.
 */
class HopByHopHeadersTest {

    // ==================== HTTP/1.1 (HttpHeaders) tests ====================

    @Test
    void testStandardHopByHopHeadersStrippedHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.CONNECTION, "keep-alive");
        headers.set("keep-alive", "timeout=5");
        headers.set(HttpHeaderNames.PROXY_AUTHENTICATE, "Basic");
        headers.set(HttpHeaderNames.PROXY_AUTHORIZATION, "Bearer token");
        headers.set(HttpHeaderNames.TE, "trailers");
        headers.set("trailers", "");
        headers.set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
        headers.set(HttpHeaderNames.UPGRADE, "h2c");
        // Non-hop-by-hop headers that must survive
        headers.set(HttpHeaderNames.CONTENT_TYPE, "text/html");
        headers.set(HttpHeaderNames.HOST, "example.com");

        HopByHopHeaders.strip(headers);

        // All hop-by-hop headers must be removed
        assertFalse(headers.contains(HttpHeaderNames.CONNECTION));
        assertFalse(headers.contains("keep-alive"));
        assertFalse(headers.contains(HttpHeaderNames.PROXY_AUTHENTICATE));
        assertFalse(headers.contains(HttpHeaderNames.PROXY_AUTHORIZATION));
        assertFalse(headers.contains(HttpHeaderNames.TE));
        assertFalse(headers.contains("trailers"));
        assertFalse(headers.contains(HttpHeaderNames.TRANSFER_ENCODING));
        assertFalse(headers.contains(HttpHeaderNames.UPGRADE));

        // Non-hop-by-hop headers must be preserved
        assertEquals("text/html", headers.get(HttpHeaderNames.CONTENT_TYPE));
        assertEquals("example.com", headers.get(HttpHeaderNames.HOST));
    }

    @Test
    void testCustomHopByHopHeadersFromConnectionHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        // Connection header lists custom headers that should also be stripped
        headers.set(HttpHeaderNames.CONNECTION, "X-Custom-Auth, X-Internal-Trace");
        headers.set("X-Custom-Auth", "secret-value");
        headers.set("X-Internal-Trace", "trace-123");
        headers.set(HttpHeaderNames.CONTENT_LENGTH, "42");

        HopByHopHeaders.strip(headers);

        // Custom hop-by-hop headers listed in Connection must be removed
        assertFalse(headers.contains("X-Custom-Auth"),
                "Custom header listed in Connection must be stripped");
        assertFalse(headers.contains("X-Internal-Trace"),
                "Custom header listed in Connection must be stripped");
        assertFalse(headers.contains(HttpHeaderNames.CONNECTION));

        // Non-hop-by-hop headers must survive
        assertEquals("42", headers.get(HttpHeaderNames.CONTENT_LENGTH));
    }

    @Test
    void testWebSocketUpgradePreservedHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.CONNECTION, "Upgrade");
        headers.set(HttpHeaderNames.UPGRADE, "websocket");
        headers.set("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==");
        headers.set("Sec-WebSocket-Version", "13");

        HopByHopHeaders.strip(headers);

        // Upgrade header MUST be preserved for WebSocket upgrades (RFC 6455)
        assertTrue(headers.contains(HttpHeaderNames.UPGRADE),
                "Upgrade header must be preserved for WebSocket upgrade");
        assertEquals("websocket", headers.get(HttpHeaderNames.UPGRADE));

        // Connection is always removed (it is hop-by-hop)
        assertFalse(headers.contains(HttpHeaderNames.CONNECTION));

        // WebSocket-specific headers are not hop-by-hop and must survive
        assertEquals("dGhlIHNhbXBsZSBub25jZQ==", headers.get("Sec-WebSocket-Key"));
        assertEquals("13", headers.get("Sec-WebSocket-Version"));
    }

    @Test
    void testNonWebSocketUpgradeStrippedHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.CONNECTION, "Upgrade");
        headers.set(HttpHeaderNames.UPGRADE, "h2c");

        HopByHopHeaders.strip(headers);

        // Non-WebSocket Upgrade must be stripped
        assertFalse(headers.contains(HttpHeaderNames.UPGRADE),
                "Non-WebSocket Upgrade header must be stripped");
        assertFalse(headers.contains(HttpHeaderNames.CONNECTION));
    }

    @Test
    void testNonHopByHopHeadersPreservedHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.CONTENT_TYPE, "application/json");
        headers.set(HttpHeaderNames.CONTENT_LENGTH, "256");
        headers.set(HttpHeaderNames.ACCEPT, "text/html");
        headers.set(HttpHeaderNames.AUTHORIZATION, "Bearer token123");
        headers.set(HttpHeaderNames.CACHE_CONTROL, "no-cache");
        headers.set("X-Forwarded-For", "192.168.1.1");
        headers.set("X-Request-ID", "req-42");

        HopByHopHeaders.strip(headers);

        // All non-hop-by-hop headers must be preserved
        assertEquals("application/json", headers.get(HttpHeaderNames.CONTENT_TYPE));
        assertEquals("256", headers.get(HttpHeaderNames.CONTENT_LENGTH));
        assertEquals("text/html", headers.get(HttpHeaderNames.ACCEPT));
        assertEquals("Bearer token123", headers.get(HttpHeaderNames.AUTHORIZATION));
        assertEquals("no-cache", headers.get(HttpHeaderNames.CACHE_CONTROL));
        assertEquals("192.168.1.1", headers.get("X-Forwarded-For"));
        assertEquals("req-42", headers.get("X-Request-ID"));
    }

    @Test
    void testEmptyHeadersHttp1() {
        HttpHeaders headers = new DefaultHttpHeaders();
        // Must not throw on empty headers
        HopByHopHeaders.strip(headers);
        assertTrue(headers.isEmpty());
    }

    @Test
    void testNullHeadersHttp1() {
        // Must not throw NullPointerException
        HopByHopHeaders.strip((HttpHeaders) null);
    }

    // ==================== HTTP/2 (Http2Headers) tests ====================

    @Test
    void testStandardHopByHopHeadersStrippedHttp2() {
        Http2Headers headers = new DefaultHttp2Headers();
        headers.set("connection", "keep-alive");
        headers.set("keep-alive", "timeout=5");
        headers.set("proxy-authenticate", "Basic");
        headers.set("proxy-authorization", "Bearer token");
        headers.set("te", "trailers");
        headers.set("trailers", "");
        headers.set("transfer-encoding", "chunked");
        headers.set("upgrade", "h2c");
        // Non-hop-by-hop headers that must survive
        headers.set("content-type", "text/html");
        headers.method("GET");
        headers.path("/index.html");

        HopByHopHeaders.strip(headers);

        // All hop-by-hop headers must be removed (RFC 9113 Section 8.2.2)
        assertNull(headers.get("connection"));
        assertNull(headers.get("keep-alive"));
        assertNull(headers.get("proxy-authenticate"));
        assertNull(headers.get("proxy-authorization"));
        assertNull(headers.get("te"));
        assertNull(headers.get("trailers"));
        assertNull(headers.get("transfer-encoding"));
        assertNull(headers.get("upgrade"));

        // Non-hop-by-hop headers must survive
        assertEquals("text/html", headers.get("content-type").toString());
        assertEquals("GET", headers.method().toString());
        assertEquals("/index.html", headers.path().toString());
    }

    @Test
    void testCustomHopByHopHeadersFromConnectionHttp2() {
        Http2Headers headers = new DefaultHttp2Headers();
        headers.set("connection", "x-custom-auth, x-internal-trace");
        headers.set("x-custom-auth", "secret-value");
        headers.set("x-internal-trace", "trace-123");
        headers.set("content-length", "42");

        HopByHopHeaders.strip(headers);

        // Custom hop-by-hop headers listed in Connection must be removed
        assertNull(headers.get("x-custom-auth"),
                "Custom header listed in Connection must be stripped");
        assertNull(headers.get("x-internal-trace"),
                "Custom header listed in Connection must be stripped");
        assertNull(headers.get("connection"));

        // Non-hop-by-hop headers must survive
        assertEquals("42", headers.get("content-length").toString());
    }

    @Test
    void testUpgradeAlwaysStrippedHttp2() {
        // In HTTP/2, Upgrade is always stripped per RFC 9113 Section 8.2.2.
        // There is no WebSocket upgrade exception for HTTP/2.
        Http2Headers headers = new DefaultHttp2Headers();
        headers.set("upgrade", "websocket");
        headers.set("connection", "upgrade");

        HopByHopHeaders.strip(headers);

        assertNull(headers.get("upgrade"),
                "Upgrade must always be stripped in HTTP/2");
        assertNull(headers.get("connection"));
    }

    @Test
    void testNullHttp2Headers() {
        // Must not throw NullPointerException
        HopByHopHeaders.strip((Http2Headers) null);
    }

    // ==================== stripForResponse() tests ====================

    @Test
    void testStripForResponsePreservesTransferEncoding() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.CONNECTION, "keep-alive");
        headers.set("keep-alive", "timeout=5");
        headers.set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
        headers.set(HttpHeaderNames.CONTENT_TYPE, "text/html");

        HopByHopHeaders.stripForResponse(headers);

        // Transfer-Encoding must be preserved for responses
        assertTrue(headers.contains(HttpHeaderNames.TRANSFER_ENCODING),
                "Transfer-Encoding must be preserved in response path");
        assertEquals("chunked", headers.get(HttpHeaderNames.TRANSFER_ENCODING));

        // Other hop-by-hop headers are still stripped
        assertFalse(headers.contains(HttpHeaderNames.CONNECTION));
        assertFalse(headers.contains("keep-alive"));

        // Non-hop-by-hop headers survive
        assertEquals("text/html", headers.get(HttpHeaderNames.CONTENT_TYPE));
    }

    @Test
    void testStripRequestRemovesTransferEncoding() {
        HttpHeaders headers = new DefaultHttpHeaders();
        headers.set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");

        HopByHopHeaders.strip(headers);

        // strip() (request path) must remove Transfer-Encoding
        assertFalse(headers.contains(HttpHeaderNames.TRANSFER_ENCODING),
                "Transfer-Encoding must be removed in request path");
    }
}
