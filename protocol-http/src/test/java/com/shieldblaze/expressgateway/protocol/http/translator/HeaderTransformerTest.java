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

import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http3.DefaultHttp3Headers;
import io.netty.handler.codec.http3.Http3Headers;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Comprehensive tests for {@link HeaderTransformer} covering all 6 protocol
 * translation directions and RFC compliance.
 */
class HeaderTransformerTest {

    // ═══════════════════════════════════════════════════════════════════════
    // H1 → H2
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H1ToH2 {

        @Test
        void requestPseudoHeaders() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/api/data?q=1");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.ACCEPT, "application/json");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertEquals("GET", h2.method().toString());
            assertEquals("/api/data?q=1", h2.path().toString());
            assertEquals("https", h2.scheme().toString());
            assertEquals("example.com", h2.authority().toString());
            assertEquals("application/json", h2.get("accept").toString());
        }

        @Test
        void requestSchemeHttp() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/submit");
            request.headers().set(HttpHeaderNames.HOST, "localhost");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, false);

            assertEquals("http", h2.scheme().toString());
        }

        @Test
        void hopByHopHeadersStripped() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.CONNECTION, "keep-alive");
            request.headers().set("keep-alive", "timeout=5");
            request.headers().set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
            request.headers().set(HttpHeaderNames.UPGRADE, "h2c");
            request.headers().set(HttpHeaderNames.TE, "trailers");
            request.headers().set("proxy-connection", "keep-alive");
            request.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/html");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertNull(h2.get("connection"));
            assertNull(h2.get("keep-alive"));
            assertNull(h2.get("transfer-encoding"));
            assertNull(h2.get("upgrade"));
            // TE: trailers is now preserved per RFC 9113 Section 8.2.2
            assertEquals("trailers", h2.get("te").toString());
            assertNull(h2.get("proxy-connection"));
            // Non-hop-by-hop preserved
            assertEquals("text/html", h2.get("content-type").toString());
        }

        @Test
        void hostNotDuplicatedAsRegularHeader() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com:8080");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertEquals("example.com:8080", h2.authority().toString());
            // Host should not appear as a regular header in H2
            assertNull(h2.get("host"));
        }

        @Test
        void cookieSplitting() {
            // RFC 9113 Section 8.2.3: Combined Cookie in H1 → split in H2
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.COOKIE, "a=1; b=2; c=3");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            // Each cookie pair should be a separate entry
            java.util.List<CharSequence> cookies = h2.getAll("cookie");
            assertNotNull(cookies);
            assertEquals(3, cookies.size());
            assertEquals("a=1", cookies.get(0).toString());
            assertEquals("b=2", cookies.get(1).toString());
            assertEquals("c=3", cookies.get(2).toString());
        }

        @Test
        void singleCookieNotSplit() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.COOKIE, "session=abc123");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            java.util.List<CharSequence> cookies = h2.getAll("cookie");
            assertEquals(1, cookies.size());
            assertEquals("session=abc123", cookies.getFirst().toString());
        }

        @Test
        void responseTranslation() {
            HttpResponse response = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "application/json");
            response.headers().set(HttpHeaderNames.CONTENT_LENGTH, "256");
            response.headers().set(HttpHeaderNames.CONNECTION, "keep-alive");

            Http2Headers h2 = HeaderTransformer.h1ResponseToH2(response);

            assertEquals("200", h2.status().toString());
            assertEquals("application/json", h2.get("content-type").toString());
            assertEquals("256", h2.get("content-length").toString());
            assertNull(h2.get("connection"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H1 → H3
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H1ToH3 {

        @Test
        void requestPseudoHeaders() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/upload");
            request.headers().set(HttpHeaderNames.HOST, "cdn.example.com");
            request.headers().set(HttpHeaderNames.CONTENT_TYPE, "multipart/form-data");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, true);

            assertEquals("POST", h3.method().toString());
            assertEquals("/upload", h3.path().toString());
            assertEquals("https", h3.scheme().toString());
            assertEquals("cdn.example.com", h3.authority().toString());
            assertEquals("multipart/form-data", h3.get("content-type").toString());
        }

        @Test
        void hopByHopStripped() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "x.com");
            request.headers().set(HttpHeaderNames.CONNECTION, "keep-alive");
            request.headers().set(HttpHeaderNames.TRANSFER_ENCODING, "chunked");
            request.headers().set("x-custom", "preserved");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, false);

            assertNull(h3.get("connection"));
            assertNull(h3.get("transfer-encoding"));
            assertEquals("preserved", h3.get("x-custom").toString());
        }

        @Test
        void responseTranslation() {
            HttpResponse response = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.NOT_FOUND);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain");

            Http3Headers h3 = HeaderTransformer.h1ResponseToH3(response);

            assertEquals("404", h3.status().toString());
            assertEquals("text/plain", h3.get("content-type").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H2 → H1
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H2ToH1 {

        @Test
        void requestConversion() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/resource/42");
            h2.scheme("https");
            h2.authority("api.example.com");
            h2.set("accept", "application/json");
            h2.set("x-custom", "value");

            HttpMethod method = HeaderTransformer.h2RequestMethod(h2);
            String path = HeaderTransformer.h2RequestPath(h2);
            HttpHeaders h1 = HeaderTransformer.h2RequestToH1Headers(h2);

            assertEquals(HttpMethod.GET, method);
            assertEquals("/resource/42", path);
            assertEquals("api.example.com", h1.get(HttpHeaderNames.HOST));
            assertEquals("application/json", h1.get("accept"));
            assertEquals("value", h1.get("x-custom"));
        }

        @Test
        void pseudoHeadersNotInH1() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("POST");
            h2.path("/submit");
            h2.scheme("https");
            h2.authority("example.com");
            h2.set("content-type", "text/plain");

            HttpHeaders h1 = HeaderTransformer.h2RequestToH1Headers(h2);

            // Pseudo-headers must not leak into H1 regular headers
            assertNull(h1.get(":method"));
            assertNull(h1.get(":path"));
            assertNull(h1.get(":scheme"));
            assertNull(h1.get(":authority"));
            // :authority becomes Host
            assertEquals("example.com", h1.get(HttpHeaderNames.HOST));
        }

        @Test
        void cookieMerging() {
            // RFC 9113 Section 8.2.3: Split cookies in H2 → merged in H1
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");
            h2.authority("example.com");
            h2.add("cookie", "a=1");
            h2.add("cookie", "b=2");
            h2.add("cookie", "c=3");

            HttpHeaders h1 = HeaderTransformer.h2RequestToH1Headers(h2);

            assertEquals("a=1; b=2; c=3", h1.get("cookie"));
        }

        @Test
        void singleCookieNotAffected() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");
            h2.add("cookie", "session=xyz");

            HttpHeaders h1 = HeaderTransformer.h2RequestToH1Headers(h2);

            assertEquals("session=xyz", h1.get("cookie"));
        }

        @Test
        void missingMethodThrows() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.path("/");

            assertThrows(IllegalArgumentException.class, () -> HeaderTransformer.h2RequestMethod(h2));
        }

        @Test
        void missingPathThrows() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");

            assertThrows(IllegalArgumentException.class, () -> HeaderTransformer.h2RequestPath(h2));
        }

        @Test
        void responseConversion() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.status("200");
            h2.set("content-type", "application/json");
            h2.set("x-trace-id", "abc");

            int status = HeaderTransformer.h2ResponseStatus(h2);
            HttpHeaders h1 = HeaderTransformer.h2ResponseToH1Headers(h2);

            assertEquals(200, status);
            assertEquals("application/json", h1.get("content-type"));
            assertEquals("abc", h1.get("x-trace-id"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H2 → H3
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H2ToH3 {

        @Test
        void requestPseudoHeadersMapped() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("PUT");
            h2.path("/api/resource");
            h2.scheme("https");
            h2.authority("example.com");
            h2.set("content-type", "application/json");

            Http3Headers h3 = HeaderTransformer.h2RequestToH3(h2);

            assertEquals("PUT", h3.method().toString());
            assertEquals("/api/resource", h3.path().toString());
            assertEquals("https", h3.scheme().toString());
            assertEquals("example.com", h3.authority().toString());
            assertEquals("application/json", h3.get("content-type").toString());
        }

        @Test
        void hopByHopStrippedDefenseInDepth() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");
            h2.set("connection", "keep-alive"); // should not be in H2 per RFC, but strip for safety
            h2.set("x-custom", "value");

            Http3Headers h3 = HeaderTransformer.h2RequestToH3(h2);

            assertNull(h3.get("connection"));
            assertEquals("value", h3.get("x-custom").toString());
        }

        @Test
        void responseMapping() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.status("404");
            h2.set("content-type", "text/plain");

            Http3Headers h3 = HeaderTransformer.h2ResponseToH3(h2);

            assertEquals("404", h3.status().toString());
            assertEquals("text/plain", h3.get("content-type").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H3 → H1
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H3ToH1 {

        @Test
        void requestConversion() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("DELETE");
            h3.path("/resource/99");
            h3.scheme("https");
            h3.authority("api.example.com:443");
            h3.set("authorization", "Bearer token");

            HttpMethod method = HeaderTransformer.h3RequestMethod(h3);
            String path = HeaderTransformer.h3RequestPath(h3);
            HttpHeaders h1 = HeaderTransformer.h3RequestToH1Headers(h3);

            assertEquals(HttpMethod.DELETE, method);
            assertEquals("/resource/99", path);
            assertEquals("api.example.com:443", h1.get(HttpHeaderNames.HOST));
            assertEquals("Bearer token", h1.get("authorization"));
        }

        @Test
        void pseudoHeadersExcluded() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");
            h3.authority("x.com");
            h3.set("accept", "text/html");

            HttpHeaders h1 = HeaderTransformer.h3RequestToH1Headers(h3);

            assertNull(h1.get(":method"));
            assertNull(h1.get(":path"));
            assertNull(h1.get(":scheme"));
            assertNull(h1.get(":authority"));
        }

        @Test
        void cookieMergingFromH3() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");
            h3.authority("x.com");
            h3.add("cookie", "x=1");
            h3.add("cookie", "y=2");

            HttpHeaders h1 = HeaderTransformer.h3RequestToH1Headers(h3);

            assertEquals("x=1; y=2", h1.get("cookie"));
        }

        @Test
        void missingMethodThrows() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.path("/");

            assertThrows(IllegalArgumentException.class, () -> HeaderTransformer.h3RequestMethod(h3));
        }

        @Test
        void responseConversion() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.status("201");
            h3.set("location", "/resource/new");

            int status = HeaderTransformer.h3ResponseStatus(h3);
            HttpHeaders h1 = HeaderTransformer.h3ResponseToH1Headers(h3);

            assertEquals(201, status);
            assertEquals("/resource/new", h1.get("location"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // H3 → H2
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class H3ToH2 {

        @Test
        void requestMapping() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.method("PATCH");
            h3.path("/api/update");
            h3.scheme("https");
            h3.authority("example.com");
            h3.set("content-type", "application/merge-patch+json");

            Http2Headers h2 = HeaderTransformer.h3RequestToH2(h3);

            assertEquals("PATCH", h2.method().toString());
            assertEquals("/api/update", h2.path().toString());
            assertEquals("https", h2.scheme().toString());
            assertEquals("example.com", h2.authority().toString());
            assertEquals("application/merge-patch+json", h2.get("content-type").toString());
        }

        @Test
        void responseMapping() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.status("503");
            h3.set("retry-after", "30");

            Http2Headers h2 = HeaderTransformer.h3ResponseToH2(h3);

            assertEquals("503", h2.status().toString());
            assertEquals("30", h2.get("retry-after").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Via header
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class ViaHeader {

        @Test
        void viaValueForH1() {
            assertEquals("1.1 expressgateway", HeaderTransformer.viaValue(ProtocolVersion.HTTP_1_1));
        }

        @Test
        void viaValueForH2() {
            assertEquals("2.0 expressgateway", HeaderTransformer.viaValue(ProtocolVersion.HTTP_2));
        }

        @Test
        void viaValueForH3() {
            assertEquals("3.0 expressgateway", HeaderTransformer.viaValue(ProtocolVersion.HTTP_3));
        }

        @Test
        void addViaToH1Headers() {
            HttpHeaders headers = new DefaultHttpHeaders();
            HeaderTransformer.addVia(headers, ProtocolVersion.HTTP_2);
            assertEquals("2.0 expressgateway", headers.get("via"));
        }

        @Test
        void addViaToH2Headers() {
            Http2Headers headers = new DefaultHttp2Headers();
            HeaderTransformer.addVia(headers, ProtocolVersion.HTTP_1_1);
            assertEquals("1.1 expressgateway", headers.get("via").toString());
        }

        @Test
        void addViaToH3Headers() {
            Http3Headers headers = new DefaultHttp3Headers();
            HeaderTransformer.addVia(headers, ProtocolVersion.HTTP_3);
            assertEquals("3.0 expressgateway", headers.get("via").toString());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Trailer conversion
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class Trailers {

        @Test
        void h1TrailersToH2() {
            HttpHeaders h1 = new DefaultHttpHeaders();
            h1.set("x-checksum", "abc123");
            h1.set("grpc-status", "0");
            h1.set("connection", "close"); // hop-by-hop, must be stripped

            Http2Headers h2 = HeaderTransformer.h1TrailersToH2(h1);

            assertEquals("abc123", h2.get("x-checksum").toString());
            assertEquals("0", h2.get("grpc-status").toString());
            assertNull(h2.get("connection"));
        }

        @Test
        void h1TrailersToH3() {
            HttpHeaders h1 = new DefaultHttpHeaders();
            h1.set("x-checksum", "def456");
            h1.set("transfer-encoding", "chunked"); // hop-by-hop

            Http3Headers h3 = HeaderTransformer.h1TrailersToH3(h1);

            assertEquals("def456", h3.get("x-checksum").toString());
            assertNull(h3.get("transfer-encoding"));
        }

        @Test
        void h2TrailersToH1() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.set("grpc-status", "0");
            h2.set("grpc-message", "OK");

            HttpHeaders h1 = HeaderTransformer.h2TrailersToH1(h2);

            assertEquals("0", h1.get("grpc-status"));
            assertEquals("OK", h1.get("grpc-message"));
        }

        @Test
        void h2TrailersToH3() {
            Http2Headers h2 = new DefaultHttp2Headers();
            h2.set("x-digest", "sha256=abc");

            Http3Headers h3 = HeaderTransformer.h2TrailersToH3(h2);

            assertEquals("sha256=abc", h3.get("x-digest").toString());
        }

        @Test
        void h3TrailersToH1() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.set("grpc-status", "2");
            h3.set("grpc-message", "error");

            HttpHeaders h1 = HeaderTransformer.h3TrailersToH1(h3);

            assertEquals("2", h1.get("grpc-status"));
            assertEquals("error", h1.get("grpc-message"));
        }

        @Test
        void h3TrailersToH2() {
            Http3Headers h3 = new DefaultHttp3Headers();
            h3.set("x-hash", "md5=xyz");

            Http2Headers h2 = HeaderTransformer.h3TrailersToH2(h3);

            assertEquals("md5=xyz", h2.get("x-hash").toString());
        }

        @Test
        void pseudoHeadersStrippedFromH2Trailers() {
            // Test that pseudo-headers in H2 trailers are stripped when converting to H1.
            // (H1 HttpHeaders won't allow creating pseudo-headers, so we test H2->H1 direction.)
            Http2Headers h2Trailers = new DefaultHttp2Headers();
            h2Trailers.status("200"); // pseudo-header that must be stripped in trailers
            h2Trailers.set("x-valid", "ok");

            HttpHeaders h1 = HeaderTransformer.h2TrailersToH1(h2Trailers);

            assertNull(h1.get(":status"));
            assertEquals("ok", h1.get("x-valid"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // RFC Compliance Fixes
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class RfcComplianceFixes {

        // --- 5a: Connection header hop-by-hop stripping ---

        @Test
        void connectionHeaderListedHeaders_strippedAsHopByHop() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.CONNECTION, "x-custom-hop, keep-alive");
            request.headers().set("x-custom-hop", "should-be-stripped");
            request.headers().set("x-regular", "should-be-kept");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertNull(h2.get("x-custom-hop"),
                    "Headers listed in Connection value must be stripped as hop-by-hop");
            assertNull(h2.get("connection"));
            assertEquals("should-be-kept", h2.get("x-regular").toString());
        }

        @Test
        void connectionHeaderListedHeaders_strippedForH3() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.CONNECTION, "x-private");
            request.headers().set("x-private", "stripped");
            request.headers().set("x-public", "kept");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, true);

            assertNull(h3.get("x-private"));
            assertEquals("kept", h3.get("x-public").toString());
        }

        // --- 5b: proxy-authenticate and proxy-authorization are end-to-end ---

        @Test
        void proxyAuthenticate_preserved_h1ToH2() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.PROXY_AUTHENTICATE, "Basic realm=proxy");
            request.headers().set(HttpHeaderNames.PROXY_AUTHORIZATION, "Basic dXNlcjpwYXNz");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertNotNull(h2.get("proxy-authenticate"),
                    "proxy-authenticate is end-to-end per RFC 9110 S11.7, must be preserved");
            assertNotNull(h2.get("proxy-authorization"),
                    "proxy-authorization is end-to-end per RFC 9110 S11.7, must be preserved");
        }

        @Test
        void proxyAuthenticate_preserved_h1ToH3() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.PROXY_AUTHORIZATION, "Bearer token");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, true);

            assertNotNull(h3.get("proxy-authorization"),
                    "proxy-authorization must be forwarded");
        }

        // --- 5c: TE: trailers preserved for H2 ---

        @Test
        void teTrailers_preserved_h1ToH2() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.TE, "trailers");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertEquals("trailers", h2.get("te").toString(),
                    "TE: trailers must be preserved per RFC 9113 Section 8.2.2");
        }

        @Test
        void teNonTrailers_stripped_h1ToH2() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.TE, "gzip");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertNull(h2.get("te"),
                    "TE values other than 'trailers' must be stripped for H2");
        }

        @Test
        void teTrailers_preserved_h1ToH3() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
            request.headers().set(HttpHeaderNames.HOST, "example.com");
            request.headers().set(HttpHeaderNames.TE, "trailers");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, true);

            assertEquals("trailers", h3.get("te").toString());
        }

        // --- 5d: CONNECT method handling ---

        @Test
        void connectMethod_h1ToH2_onlyMethodAndAuthority() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.CONNECT, "proxy.example.com:443");
            request.headers().set(HttpHeaderNames.HOST, "proxy.example.com:443");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertEquals("CONNECT", h2.method().toString());
            assertEquals("proxy.example.com:443", h2.authority().toString());
            assertNull(h2.scheme(), "CONNECT must NOT have :scheme per RFC 9113 S8.5");
            assertNull(h2.path(), "CONNECT must NOT have :path per RFC 9113 S8.5");
        }

        @Test
        void connectMethod_h1ToH3_onlyMethodAndAuthority() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.CONNECT, "proxy.example.com:443");
            request.headers().set(HttpHeaderNames.HOST, "proxy.example.com:443");

            Http3Headers h3 = HeaderTransformer.h1RequestToH3(request, true);

            assertEquals("CONNECT", h3.method().toString());
            assertEquals("proxy.example.com:443", h3.authority().toString());
            assertNull(h3.scheme(), "CONNECT must NOT have :scheme per RFC 9114 S4.4");
            assertNull(h3.path(), "CONNECT must NOT have :path per RFC 9114 S4.4");
        }

        @Test
        void nonConnectMethod_stillHasSchemeAndPath() {
            HttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/api/test");
            request.headers().set(HttpHeaderNames.HOST, "example.com");

            Http2Headers h2 = HeaderTransformer.h1RequestToH2(request, true);

            assertNotNull(h2.scheme());
            assertNotNull(h2.path());
        }

        // --- 5e: Header value sanitization ---

        @Test
        void h2ToH1_headerInjection_blocked() {
            Http2Headers h2 = new DefaultHttp2Headers(false);
            h2.method("GET");
            h2.path("/");
            h2.scheme("https");
            h2.authority("example.com");
            h2.set("x-safe", "normal-value");
            h2.set("x-evil", "value\r\nX-Injected: attack");

            HttpHeaders h1 = HeaderTransformer.h2RequestToH1Headers(h2);

            assertEquals("normal-value", h1.get("x-safe"));
            assertNull(h1.get("x-evil"),
                    "Headers with CR/LF in values must be rejected to prevent header injection");
            assertNull(h1.get("x-injected"));
        }

        @Test
        void h3ToH1_headerInjection_blocked() {
            Http3Headers h3 = new DefaultHttp3Headers(false);
            h3.method("GET");
            h3.path("/");
            h3.scheme("https");
            h3.authority("example.com");
            h3.set("x-safe", "ok");
            h3.set("x-nul", "value\0injected");

            HttpHeaders h1 = HeaderTransformer.h3RequestToH1Headers(h3);

            assertEquals("ok", h1.get("x-safe"));
            assertNull(h1.get("x-nul"),
                    "Headers with NUL in values must be rejected");
        }

        @Test
        void h2ResponseToH1_headerInjection_blocked() {
            Http2Headers h2 = new DefaultHttp2Headers(false);
            h2.status("200");
            h2.set("x-safe", "ok");
            h2.set("x-bad", "value\ninjection");

            HttpHeaders h1 = HeaderTransformer.h2ResponseToH1Headers(h2);

            assertEquals("ok", h1.get("x-safe"));
            assertNull(h1.get("x-bad"));
        }

        @Test
        void h3ResponseToH1_headerInjection_blocked() {
            Http3Headers h3 = new DefaultHttp3Headers(false);
            h3.status("200");
            h3.set("x-safe", "ok");
            h3.set("x-cr", "value\rinjection");

            HttpHeaders h1 = HeaderTransformer.h3ResponseToH1Headers(h3);

            assertEquals("ok", h1.get("x-safe"));
            assertNull(h1.get("x-cr"));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Validation
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class Validation {

        @Test
        void validateH2RequestPseudoHeaders() {
            Http2Headers valid = new DefaultHttp2Headers();
            valid.method("GET");
            valid.path("/");
            valid.scheme("https");
            assertTrue(HeaderTransformer.validateH2RequestPseudoHeaders(valid));

            Http2Headers noMethod = new DefaultHttp2Headers();
            noMethod.path("/");
            noMethod.scheme("https");
            assertFalse(HeaderTransformer.validateH2RequestPseudoHeaders(noMethod));

            Http2Headers noPath = new DefaultHttp2Headers();
            noPath.method("GET");
            noPath.scheme("https");
            assertFalse(HeaderTransformer.validateH2RequestPseudoHeaders(noPath));

            Http2Headers noScheme = new DefaultHttp2Headers();
            noScheme.method("GET");
            noScheme.path("/");
            assertFalse(HeaderTransformer.validateH2RequestPseudoHeaders(noScheme));
        }

        @Test
        void validateH3RequestPseudoHeaders() {
            Http3Headers valid = new DefaultHttp3Headers();
            valid.method("POST");
            valid.path("/submit");
            valid.scheme("https");
            assertTrue(HeaderTransformer.validateH3RequestPseudoHeaders(valid));

            Http3Headers noMethod = new DefaultHttp3Headers();
            noMethod.path("/");
            noMethod.scheme("https");
            assertFalse(HeaderTransformer.validateH3RequestPseudoHeaders(noMethod));
        }

        @Test
        void isPseudoHeader() {
            assertTrue(HeaderTransformer.isPseudoHeader(":method"));
            assertTrue(HeaderTransformer.isPseudoHeader(":status"));
            assertTrue(HeaderTransformer.isPseudoHeader(":path"));
            assertFalse(HeaderTransformer.isPseudoHeader("content-type"));
            assertFalse(HeaderTransformer.isPseudoHeader("host"));
        }

        @Test
        void isHopByHop() {
            assertTrue(HeaderTransformer.isHopByHop("connection"));
            assertTrue(HeaderTransformer.isHopByHop("Connection"));
            assertTrue(HeaderTransformer.isHopByHop("keep-alive"));
            assertTrue(HeaderTransformer.isHopByHop("transfer-encoding"));
            assertTrue(HeaderTransformer.isHopByHop("upgrade"));
            assertTrue(HeaderTransformer.isHopByHop("te"));
            assertTrue(HeaderTransformer.isHopByHop("proxy-connection"));
            assertFalse(HeaderTransformer.isHopByHop("content-type"));
            assertFalse(HeaderTransformer.isHopByHop("authorization"));
            // proxy-authenticate and proxy-authorization are end-to-end per RFC 9110 S11.7
            assertFalse(HeaderTransformer.isHopByHop("proxy-authenticate"));
            assertFalse(HeaderTransformer.isHopByHop("proxy-authorization"));
        }

        @Test
        void containsProhibitedChars() {
            assertFalse(HeaderTransformer.containsProhibitedChars(null));
            assertFalse(HeaderTransformer.containsProhibitedChars("normal-value"));
            assertFalse(HeaderTransformer.containsProhibitedChars(""));
            assertTrue(HeaderTransformer.containsProhibitedChars("value\0injected"));
            assertTrue(HeaderTransformer.containsProhibitedChars("value\rinjected"));
            assertTrue(HeaderTransformer.containsProhibitedChars("value\ninjected"));
            assertTrue(HeaderTransformer.containsProhibitedChars("\r\nX-Evil: header"));
        }
    }
}
