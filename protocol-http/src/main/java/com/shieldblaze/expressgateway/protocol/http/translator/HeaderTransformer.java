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

import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http3.DefaultHttp3Headers;
import io.netty.handler.codec.http3.Http3Headers;

import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * RFC-compliant header transformation utilities for protocol translation between
 * HTTP/1.1 (RFC 9112), HTTP/2 (RFC 9113), and HTTP/3 (RFC 9114).
 *
 * <p>Implements:
 * <ul>
 *   <li>RFC 9113 Section 8.2: HTTP/2 header field validation and pseudo-header conversion</li>
 *   <li>RFC 9114 Section 4.2: HTTP/3 header field requirements</li>
 *   <li>RFC 9110 Section 7.6.1: Hop-by-hop header removal for proxies</li>
 *   <li>RFC 9113 Section 8.2.3: Cookie header merging/splitting</li>
 *   <li>RFC 9110 Section 7.6.3: Via header addition</li>
 * </ul>
 *
 * <p>Thread-safety: All methods are stateless and safe for concurrent use.</p>
 */
public final class HeaderTransformer {

    /**
     * Hop-by-hop headers that MUST be stripped when translating between protocols.
     * Per RFC 9110 Section 7.6.1 and RFC 9113 Section 8.2.2, these are
     * connection-specific headers that MUST NOT appear in HTTP/2 or HTTP/3 messages.
     *
     * <p>Note: proxy-authenticate and proxy-authorization are NOT hop-by-hop.
     * Per RFC 9110 Section 11.7, they are end-to-end headers and MUST be forwarded.</p>
     */
    private static final Set<String> HOP_BY_HOP = Set.of(
            "connection",
            "keep-alive",
            "te",
            "trailers",
            "transfer-encoding",
            "upgrade",
            "proxy-connection"
    );

    /**
     * HTTP/2/HTTP/3 pseudo-header names. These MUST NOT be forwarded as regular
     * headers when translating to HTTP/1.1. Per RFC 9113 Section 8.1, pseudo-header
     * fields are not HTTP header fields and MUST NOT appear in HTTP/1.1 messages.
     */
    private static final Set<String> PSEUDO_HEADERS = Set.of(
            ":method",
            ":path",
            ":scheme",
            ":authority",
            ":status",
            ":protocol"
    );

    private HeaderTransformer() {
    }

    /**
     * Collects the full set of hop-by-hop header names per RFC 9110 Section 7.6.1.
     * Parses the Connection header value and adds any listed header names to the
     * static HOP_BY_HOP set. This ensures that application-designated hop-by-hop
     * headers (e.g., "Connection: x-custom, keep-alive") are also stripped.
     *
     * @param headers the HTTP/1.1 headers to inspect
     * @return the union of static hop-by-hop names and Connection-listed names
     */
    private static Set<String> collectHopByHopHeaders(HttpHeaders headers) {
        String connectionValue = headers.get(HttpHeaderNames.CONNECTION);
        if (connectionValue == null || connectionValue.isEmpty()) {
            return HOP_BY_HOP;
        }
        Set<String> expanded = new HashSet<>(HOP_BY_HOP);
        for (String token : connectionValue.split(",")) {
            String trimmed = token.trim().toLowerCase();
            if (!trimmed.isEmpty()) {
                expanded.add(trimmed);
            }
        }
        return expanded;
    }

    // ── H1 → H2 ─────────────────────────────────────────────────────────────

    /**
     * Converts HTTP/1.1 request headers to HTTP/2 pseudo-headers and regular headers.
     *
     * <p>Per RFC 9113 Section 8.3.1:
     * <ul>
     *   <li>{@code :method} from request method</li>
     *   <li>{@code :scheme} from TLS context or X-Forwarded-Proto</li>
     *   <li>{@code :path} from request URI</li>
     *   <li>{@code :authority} from Host header</li>
     * </ul>
     *
     * <p>Hop-by-hop headers are stripped. The Host header is consumed into
     * :authority and NOT duplicated as a regular header (RFC 9113 Section 8.3.1).</p>
     *
     * @param request   the HTTP/1.1 request
     * @param isTls     whether the frontend connection uses TLS (determines :scheme)
     * @return HTTP/2 headers with pseudo-headers and cleaned regular headers
     */
    public static Http2Headers h1RequestToH2(HttpRequest request, boolean isTls) {
        Http2Headers h2 = new DefaultHttp2Headers();

        h2.method(request.method().asciiName());

        // RFC 9113 Section 8.5: CONNECT requests MUST only have :method and :authority.
        // :scheme and :path MUST NOT be included.
        if (HttpMethod.CONNECT.equals(request.method())) {
            String host = request.headers().get(HttpHeaderNames.HOST);
            if (host != null) {
                h2.authority(host);
            }
            copyH1ToH2Regular(request.headers(), h2);
            return h2;
        }

        h2.path(request.uri());
        h2.scheme(isTls ? "https" : "http");

        String host = request.headers().get(HttpHeaderNames.HOST);
        if (host != null) {
            h2.authority(host);
        }

        copyH1ToH2Regular(request.headers(), h2);
        return h2;
    }

    /**
     * Converts HTTP/1.1 response headers to HTTP/2 headers with :status pseudo-header.
     *
     * @param response the HTTP/1.1 response
     * @return HTTP/2 headers with :status and cleaned regular headers
     */
    public static Http2Headers h1ResponseToH2(HttpResponse response) {
        Http2Headers h2 = new DefaultHttp2Headers();
        h2.status(String.valueOf(response.status().code()));
        copyH1ToH2Regular(response.headers(), h2);
        return h2;
    }

    // ── H1 → H3 ─────────────────────────────────────────────────────────────

    /**
     * Converts HTTP/1.1 request headers to HTTP/3 headers.
     *
     * <p>Per RFC 9114 Section 4.3.1, HTTP/3 uses the same pseudo-headers as HTTP/2.
     * The header encoding is QPACK (RFC 9204) instead of HPACK, but the logical
     * structure is identical. Connection-specific headers are stripped.</p>
     *
     * @param request the HTTP/1.1 request
     * @param isTls   whether the frontend uses TLS (HTTP/3 always implies TLS over QUIC,
     *                but the scheme may differ for protocol translation from non-TLS H1)
     * @return HTTP/3 headers with pseudo-headers and cleaned regular headers
     */
    public static Http3Headers h1RequestToH3(HttpRequest request, boolean isTls) {
        Http3Headers h3 = new DefaultHttp3Headers();

        h3.method(request.method().asciiName());

        // RFC 9114 Section 4.4: CONNECT requests MUST only have :method and :authority.
        if (HttpMethod.CONNECT.equals(request.method())) {
            String host = request.headers().get(HttpHeaderNames.HOST);
            if (host != null) {
                h3.authority(host);
            }
            copyH1ToH3Regular(request.headers(), h3);
            return h3;
        }

        h3.path(request.uri());
        h3.scheme(isTls ? "https" : "http");

        String host = request.headers().get(HttpHeaderNames.HOST);
        if (host != null) {
            h3.authority(host);
        }

        copyH1ToH3Regular(request.headers(), h3);
        return h3;
    }

    /**
     * Converts HTTP/1.1 response headers to HTTP/3 headers with :status.
     *
     * @param response the HTTP/1.1 response
     * @return HTTP/3 headers with :status and cleaned regular headers
     */
    public static Http3Headers h1ResponseToH3(HttpResponse response) {
        Http3Headers h3 = new DefaultHttp3Headers();
        h3.status(String.valueOf(response.status().code()));
        copyH1ToH3Regular(response.headers(), h3);
        return h3;
    }

    // ── H2 → H1 ─────────────────────────────────────────────────────────────

    /**
     * Extracts the HTTP method from HTTP/2 pseudo-headers.
     *
     * @param h2 the HTTP/2 request headers
     * @return the HTTP method
     * @throws IllegalArgumentException if :method is missing
     */
    public static HttpMethod h2RequestMethod(Http2Headers h2) {
        CharSequence method = h2.method();
        if (method == null) {
            throw new IllegalArgumentException("Missing :method pseudo-header");
        }
        return HttpMethod.valueOf(method.toString());
    }

    /**
     * Extracts the request URI from HTTP/2 pseudo-headers.
     *
     * @param h2 the HTTP/2 request headers
     * @return the request path (URI)
     * @throws IllegalArgumentException if :path is missing
     */
    public static String h2RequestPath(Http2Headers h2) {
        CharSequence path = h2.path();
        if (path == null) {
            throw new IllegalArgumentException("Missing :path pseudo-header");
        }
        return path.toString();
    }

    /**
     * Converts HTTP/2 request headers to HTTP/1.1 headers.
     *
     * <p>Per RFC 9113 Section 8.3.1:
     * <ul>
     *   <li>:method and :path form the request line (handled by caller)</li>
     *   <li>:authority becomes the Host header</li>
     *   <li>:scheme is dropped (implicit in HTTP/1.1 connection)</li>
     *   <li>Pseudo-headers are NOT forwarded as regular headers</li>
     * </ul>
     *
     * <p>RFC 9113 Section 8.2.3: Multiple Cookie headers in HTTP/2 are concatenated
     * with "; " to form a single Cookie header in HTTP/1.1.</p>
     *
     * @param h2 the HTTP/2 headers
     * @return HTTP/1.1 headers with Host set from :authority and cookies merged
     */
    public static HttpHeaders h2RequestToH1Headers(Http2Headers h2) {
        HttpHeaders h1 = new DefaultHttpHeaders();

        CharSequence authority = h2.authority();
        if (authority != null) {
            h1.set(HttpHeaderNames.HOST, authority.toString());
        }

        copyH2ToH1Regular(h2, h1);
        mergeCookiesForH1(h2, h1);
        return h1;
    }

    /**
     * Extracts HTTP status code from HTTP/2 response :status pseudo-header.
     *
     * @param h2 the HTTP/2 response headers
     * @return the status code as an integer
     * @throws IllegalArgumentException if :status is missing or not a valid integer
     */
    public static int h2ResponseStatus(Http2Headers h2) {
        CharSequence status = h2.status();
        if (status == null) {
            throw new IllegalArgumentException("Missing :status pseudo-header");
        }
        return Integer.parseInt(status.toString());
    }

    /**
     * Converts HTTP/2 response headers to HTTP/1.1 headers.
     *
     * @param h2 the HTTP/2 response headers
     * @return HTTP/1.1 response headers with hop-by-hop stripped and cookies merged
     */
    public static HttpHeaders h2ResponseToH1Headers(Http2Headers h2) {
        HttpHeaders h1 = new DefaultHttpHeaders();
        copyH2ToH1Regular(h2, h1);
        mergeCookiesForH1(h2, h1);
        return h1;
    }

    // ── H2 → H3 ─────────────────────────────────────────────────────────────

    /**
     * Converts HTTP/2 request headers to HTTP/3 request headers.
     *
     * <p>Both HTTP/2 and HTTP/3 share the same pseudo-header semantics (RFC 9113
     * Section 8.3.1, RFC 9114 Section 4.3.1). The difference is header encoding:
     * HPACK for HTTP/2, QPACK for HTTP/3. At the logical header level, this is
     * a direct mapping.</p>
     *
     * <p>Connection-specific headers that may have leaked from a non-compliant
     * HTTP/2 peer are stripped for defense in depth.</p>
     *
     * @param h2 the HTTP/2 headers
     * @return HTTP/3 headers with the same pseudo-headers and regular headers
     */
    public static Http3Headers h2RequestToH3(Http2Headers h2) {
        Http3Headers h3 = new DefaultHttp3Headers();

        if (h2.method() != null) h3.method(h2.method());
        if (h2.path() != null) h3.path(h2.path());
        if (h2.scheme() != null) h3.scheme(h2.scheme());
        if (h2.authority() != null) h3.authority(h2.authority());

        copyH2ToH3Regular(h2, h3);
        return h3;
    }

    /**
     * Converts HTTP/2 response headers to HTTP/3 response headers.
     *
     * @param h2 the HTTP/2 response headers
     * @return HTTP/3 headers with :status and cleaned regular headers
     */
    public static Http3Headers h2ResponseToH3(Http2Headers h2) {
        Http3Headers h3 = new DefaultHttp3Headers();

        if (h2.status() != null) h3.status(h2.status());

        copyH2ToH3Regular(h2, h3);
        return h3;
    }

    // ── H3 → H1 ─────────────────────────────────────────────────────────────

    /**
     * Extracts the HTTP method from HTTP/3 pseudo-headers.
     *
     * @param h3 the HTTP/3 request headers
     * @return the HTTP method
     * @throws IllegalArgumentException if :method is missing
     */
    public static HttpMethod h3RequestMethod(Http3Headers h3) {
        CharSequence method = h3.method();
        if (method == null) {
            throw new IllegalArgumentException("Missing :method pseudo-header");
        }
        return HttpMethod.valueOf(method.toString());
    }

    /**
     * Extracts the request URI from HTTP/3 pseudo-headers.
     *
     * @param h3 the HTTP/3 request headers
     * @return the request path
     * @throws IllegalArgumentException if :path is missing
     */
    public static String h3RequestPath(Http3Headers h3) {
        CharSequence path = h3.path();
        if (path == null) {
            throw new IllegalArgumentException("Missing :path pseudo-header");
        }
        return path.toString();
    }

    /**
     * Converts HTTP/3 request headers to HTTP/1.1 headers.
     *
     * <p>Mirrors the H2 -> H1 path: :authority becomes Host, pseudo-headers
     * are dropped, cookies from multiple QPACK entries are merged.</p>
     *
     * @param h3 the HTTP/3 headers
     * @return HTTP/1.1 headers
     */
    public static HttpHeaders h3RequestToH1Headers(Http3Headers h3) {
        HttpHeaders h1 = new DefaultHttpHeaders();

        CharSequence authority = h3.authority();
        if (authority != null) {
            h1.set(HttpHeaderNames.HOST, authority.toString());
        }

        copyH3ToH1Regular(h3, h1);
        mergeCookiesForH1FromH3(h3, h1);
        return h1;
    }

    /**
     * Extracts HTTP status code from HTTP/3 response :status pseudo-header.
     *
     * @param h3 the HTTP/3 response headers
     * @return the status code
     * @throws IllegalArgumentException if :status is missing
     */
    public static int h3ResponseStatus(Http3Headers h3) {
        CharSequence status = h3.status();
        if (status == null) {
            throw new IllegalArgumentException("Missing :status pseudo-header");
        }
        return Integer.parseInt(status.toString());
    }

    /**
     * Converts HTTP/3 response headers to HTTP/1.1 headers.
     *
     * @param h3 the HTTP/3 response headers
     * @return HTTP/1.1 response headers
     */
    public static HttpHeaders h3ResponseToH1Headers(Http3Headers h3) {
        HttpHeaders h1 = new DefaultHttpHeaders();
        copyH3ToH1Regular(h3, h1);
        mergeCookiesForH1FromH3(h3, h1);
        return h1;
    }

    // ── H3 → H2 ─────────────────────────────────────────────────────────────

    /**
     * Converts HTTP/3 request headers to HTTP/2 request headers.
     *
     * <p>Direct pseudo-header mapping (both protocols share the same pseudo-headers).
     * QPACK-decoded headers are re-encoded as HPACK on the HTTP/2 side by the codec.</p>
     *
     * @param h3 the HTTP/3 headers
     * @return HTTP/2 headers
     */
    public static Http2Headers h3RequestToH2(Http3Headers h3) {
        Http2Headers h2 = new DefaultHttp2Headers();

        if (h3.method() != null) h2.method(h3.method());
        if (h3.path() != null) h2.path(h3.path());
        if (h3.scheme() != null) h2.scheme(h3.scheme());
        if (h3.authority() != null) h2.authority(h3.authority());

        copyH3ToH2Regular(h3, h2);
        return h2;
    }

    /**
     * Converts HTTP/3 response headers to HTTP/2 response headers.
     *
     * @param h3 the HTTP/3 response headers
     * @return HTTP/2 headers
     */
    public static Http2Headers h3ResponseToH2(Http3Headers h3) {
        Http2Headers h2 = new DefaultHttp2Headers();

        if (h3.status() != null) h2.status(h3.status());

        copyH3ToH2Regular(h3, h2);
        return h2;
    }

    // ── Via header ────────────────────────────────────────────────────────────

    /**
     * Creates a Via header value per RFC 9110 Section 7.6.3.
     *
     * <p>Format: {@code <protocol-version> expressgateway}</p>
     * <p>Examples: "1.1 expressgateway", "2.0 expressgateway", "3.0 expressgateway"</p>
     *
     * @param sourceProtocol the protocol version of the received message
     * @return the Via header value
     */
    public static String viaValue(ProtocolVersion sourceProtocol) {
        return sourceProtocol.viaToken() + " expressgateway";
    }

    /**
     * Adds a Via header to HTTP/1.1 headers.
     *
     * @param headers        the headers to modify
     * @param sourceProtocol the protocol of the received message
     */
    public static void addVia(HttpHeaders headers, ProtocolVersion sourceProtocol) {
        headers.add("via", viaValue(sourceProtocol));
    }

    /**
     * Adds a Via header to HTTP/2 headers.
     *
     * @param headers        the headers to modify
     * @param sourceProtocol the protocol of the received message
     */
    public static void addVia(Http2Headers headers, ProtocolVersion sourceProtocol) {
        headers.add("via", viaValue(sourceProtocol));
    }

    /**
     * Adds a Via header to HTTP/3 headers.
     *
     * @param headers        the headers to modify
     * @param sourceProtocol the protocol of the received message
     */
    public static void addVia(Http3Headers headers, ProtocolVersion sourceProtocol) {
        headers.add("via", viaValue(sourceProtocol));
    }

    // ── Trailer conversion ───────────────────────────────────────────────────

    /**
     * Converts HTTP/1.1 trailing headers to HTTP/2 trailing headers.
     * Strips pseudo-headers and hop-by-hop headers from trailers per RFC 9113 Section 8.1.
     *
     * @param h1Trailers the HTTP/1.1 trailing headers
     * @return HTTP/2 trailing headers
     */
    public static Http2Headers h1TrailersToH2(HttpHeaders h1Trailers) {
        Http2Headers h2 = new DefaultHttp2Headers();
        for (Map.Entry<String, String> entry : h1Trailers) {
            String name = entry.getKey().toLowerCase();
            if (!HOP_BY_HOP.contains(name) && !PSEUDO_HEADERS.contains(name)) {
                h2.add(name, entry.getValue());
            }
        }
        return h2;
    }

    /**
     * Converts HTTP/1.1 trailing headers to HTTP/3 trailing headers.
     *
     * @param h1Trailers the HTTP/1.1 trailing headers
     * @return HTTP/3 trailing headers
     */
    public static Http3Headers h1TrailersToH3(HttpHeaders h1Trailers) {
        Http3Headers h3 = new DefaultHttp3Headers();
        for (Map.Entry<String, String> entry : h1Trailers) {
            String name = entry.getKey().toLowerCase();
            if (!HOP_BY_HOP.contains(name) && !PSEUDO_HEADERS.contains(name)) {
                h3.add(name, entry.getValue());
            }
        }
        return h3;
    }

    /**
     * Converts HTTP/2 trailing headers to HTTP/1.1 trailing headers.
     *
     * @param h2Trailers the HTTP/2 trailing headers
     * @return HTTP/1.1 trailing headers
     */
    public static HttpHeaders h2TrailersToH1(Http2Headers h2Trailers) {
        HttpHeaders h1 = new DefaultHttpHeaders();
        for (Map.Entry<CharSequence, CharSequence> entry : h2Trailers) {
            String name = entry.getKey().toString().toLowerCase();
            if (!PSEUDO_HEADERS.contains(name) && !HOP_BY_HOP.contains(name)) {
                h1.add(name, entry.getValue().toString());
            }
        }
        return h1;
    }

    /**
     * Converts HTTP/2 trailing headers to HTTP/3 trailing headers.
     *
     * @param h2Trailers the HTTP/2 trailing headers
     * @return HTTP/3 trailing headers
     */
    public static Http3Headers h2TrailersToH3(Http2Headers h2Trailers) {
        Http3Headers h3 = new DefaultHttp3Headers();
        for (Map.Entry<CharSequence, CharSequence> entry : h2Trailers) {
            String name = entry.getKey().toString();
            if (name.charAt(0) != ':' && !HOP_BY_HOP.contains(name.toLowerCase())) {
                h3.add(name, entry.getValue());
            }
        }
        return h3;
    }

    /**
     * Converts HTTP/3 trailing headers to HTTP/1.1 trailing headers.
     *
     * @param h3Trailers the HTTP/3 trailing headers
     * @return HTTP/1.1 trailing headers
     */
    public static HttpHeaders h3TrailersToH1(Http3Headers h3Trailers) {
        HttpHeaders h1 = new DefaultHttpHeaders();
        for (Map.Entry<CharSequence, CharSequence> entry : h3Trailers) {
            String name = entry.getKey().toString().toLowerCase();
            if (!PSEUDO_HEADERS.contains(name) && !HOP_BY_HOP.contains(name)) {
                h1.add(name, entry.getValue().toString());
            }
        }
        return h1;
    }

    /**
     * Converts HTTP/3 trailing headers to HTTP/2 trailing headers.
     *
     * @param h3Trailers the HTTP/3 trailing headers
     * @return HTTP/2 trailing headers
     */
    public static Http2Headers h3TrailersToH2(Http3Headers h3Trailers) {
        Http2Headers h2 = new DefaultHttp2Headers();
        for (Map.Entry<CharSequence, CharSequence> entry : h3Trailers) {
            String name = entry.getKey().toString();
            if (name.charAt(0) != ':' && !HOP_BY_HOP.contains(name.toLowerCase())) {
                h2.add(name, entry.getValue());
            }
        }
        return h2;
    }

    // ── Validation ────────────────────────────────────────────────────────────

    /**
     * Validates that HTTP/2 request headers contain the required pseudo-headers
     * per RFC 9113 Section 8.3.1.
     *
     * @param h2 the HTTP/2 request headers
     * @return {@code true} if all required pseudo-headers are present
     */
    public static boolean validateH2RequestPseudoHeaders(Http2Headers h2) {
        return h2.method() != null && h2.path() != null && h2.scheme() != null;
    }

    /**
     * Validates that HTTP/3 request headers contain the required pseudo-headers
     * per RFC 9114 Section 4.3.1.
     *
     * @param h3 the HTTP/3 request headers
     * @return {@code true} if all required pseudo-headers are present
     */
    public static boolean validateH3RequestPseudoHeaders(Http3Headers h3) {
        return h3.method() != null && h3.path() != null && h3.scheme() != null;
    }

    /**
     * Checks whether a header name is a pseudo-header (starts with ':').
     *
     * @param name the header name
     * @return {@code true} if this is a pseudo-header
     */
    public static boolean isPseudoHeader(CharSequence name) {
        return name.length() > 0 && name.charAt(0) == ':';
    }

    /**
     * Checks whether a header name is a hop-by-hop header.
     *
     * @param name the header name (case-insensitive)
     * @return {@code true} if this is a hop-by-hop header
     */
    public static boolean isHopByHop(String name) {
        return HOP_BY_HOP.contains(name.toLowerCase());
    }

    /**
     * Checks whether the given CharSequence contains characters prohibited in
     * HTTP/2 and HTTP/3 header values per RFC 9113 Section 8.2.1 and
     * RFC 9114 Section 4.2: NUL (0x00), CR (0x0d), LF (0x0a).
     *
     * @param value the header value to check
     * @return {@code true} if the value contains prohibited characters
     */
    public static boolean containsProhibitedChars(CharSequence value) {
        if (value == null) {
            return false;
        }
        for (int i = 0, len = value.length(); i < len; i++) {
            char c = value.charAt(i);
            if (c == '\0' || c == '\r' || c == '\n') {
                return true;
            }
        }
        return false;
    }

    // ── Internal copy helpers ─────────────────────────────────────────────────

    /**
     * Copies regular (non-pseudo, non-hop-by-hop) headers from H1 to H2.
     * Host is excluded because it's already consumed into :authority.
     * Cookie headers are split into individual entries per RFC 9113 Section 8.2.3.
     *
     * <p>RFC 9110 Section 7.6.1: The Connection header value names additional
     * hop-by-hop headers that must also be stripped.</p>
     *
     * <p>RFC 9113 Section 8.2.2: "te" is hop-by-hop except for the value "trailers"
     * which MUST be preserved when translating to H2.</p>
     */
    private static void copyH1ToH2Regular(HttpHeaders h1, Http2Headers h2) {
        Set<String> hopByHop = collectHopByHopHeaders(h1);
        for (Map.Entry<String, String> entry : h1) {
            String name = entry.getKey().toLowerCase();
            if ("host".equals(name)) continue;

            // RFC 9113 Section 8.2.2: TE header -- only "trailers" value is allowed in H2
            if ("te".equals(name)) {
                if ("trailers".equalsIgnoreCase(entry.getValue().trim())) {
                    h2.add("te", "trailers");
                }
                continue;
            }

            if (hopByHop.contains(name)) continue;

            // RFC 9113 Section 8.2.3: Each cookie value in HTTP/1.1 "Cookie: a=1; b=2"
            // SHOULD be split into separate header entries in HTTP/2.
            if ("cookie".equals(name)) {
                splitCookieToH2(entry.getValue(), h2);
                continue;
            }

            h2.add(name, entry.getValue());
        }
    }

    /**
     * Copies regular headers from H1 to H3, mirroring the H1-to-H2 logic.
     * Includes Connection-header-listed hop-by-hop stripping and TE: trailers handling.
     */
    private static void copyH1ToH3Regular(HttpHeaders h1, Http3Headers h3) {
        Set<String> hopByHop = collectHopByHopHeaders(h1);
        for (Map.Entry<String, String> entry : h1) {
            String name = entry.getKey().toLowerCase();
            if ("host".equals(name)) continue;

            // TE: only "trailers" is allowed (same logic as H2)
            if ("te".equals(name)) {
                if ("trailers".equalsIgnoreCase(entry.getValue().trim())) {
                    h3.add("te", "trailers");
                }
                continue;
            }

            if (hopByHop.contains(name)) continue;

            if ("cookie".equals(name)) {
                splitCookieToH3(entry.getValue(), h3);
                continue;
            }

            h3.add(name, entry.getValue());
        }
    }

    /**
     * Copies regular headers from H2 to H1. Pseudo-headers and hop-by-hop are excluded.
     * Cookie is handled separately via {@link #mergeCookiesForH1}.
     * Header values containing NUL, CR, or LF are rejected to prevent header injection.
     */
    private static void copyH2ToH1Regular(Http2Headers h2, HttpHeaders h1) {
        for (Map.Entry<CharSequence, CharSequence> entry : h2) {
            String name = entry.getKey().toString();
            String nameLower = name.toLowerCase();
            if (name.charAt(0) == ':') continue;
            if (HOP_BY_HOP.contains(nameLower)) continue;
            if ("cookie".equals(nameLower)) continue; // handled by mergeCookiesForH1
            if (containsProhibitedChars(entry.getValue())) continue; // reject header injection

            h1.add(nameLower, entry.getValue().toString());
        }
    }

    /**
     * Copies regular headers from H2 to H3. Pseudo-headers are excluded.
     */
    private static void copyH2ToH3Regular(Http2Headers h2, Http3Headers h3) {
        for (Map.Entry<CharSequence, CharSequence> entry : h2) {
            String name = entry.getKey().toString();
            if (name.charAt(0) == ':') continue;
            if (HOP_BY_HOP.contains(name.toLowerCase())) continue;

            h3.add(name, entry.getValue());
        }
    }

    /**
     * Copies regular headers from H3 to H1.
     * Header values containing NUL, CR, or LF are rejected to prevent header injection.
     */
    private static void copyH3ToH1Regular(Http3Headers h3, HttpHeaders h1) {
        for (Map.Entry<CharSequence, CharSequence> entry : h3) {
            String name = entry.getKey().toString();
            String nameLower = name.toLowerCase();
            if (name.charAt(0) == ':') continue;
            if (HOP_BY_HOP.contains(nameLower)) continue;
            if ("cookie".equals(nameLower)) continue; // handled by mergeCookiesForH1FromH3
            if (containsProhibitedChars(entry.getValue())) continue; // reject header injection

            h1.add(nameLower, entry.getValue().toString());
        }
    }

    /**
     * Copies regular headers from H3 to H2.
     */
    private static void copyH3ToH2Regular(Http3Headers h3, Http2Headers h2) {
        for (Map.Entry<CharSequence, CharSequence> entry : h3) {
            String name = entry.getKey().toString();
            if (name.charAt(0) == ':') continue;
            if (HOP_BY_HOP.contains(name.toLowerCase())) continue;

            h2.add(name, entry.getValue());
        }
    }

    // ── Cookie handling ───────────────────────────────────────────────────────

    /**
     * RFC 9113 Section 8.2.3: Splits a combined HTTP/1.1 Cookie header value
     * (e.g., "a=1; b=2") into individual HTTP/2 cookie header entries.
     * Each semicolon-separated pair becomes a separate header entry.
     */
    private static void splitCookieToH2(String cookieValue, Http2Headers h2) {
        if (cookieValue.indexOf(';') < 0) {
            h2.add("cookie", cookieValue.trim());
            return;
        }
        int start = 0;
        int len = cookieValue.length();
        while (start < len) {
            int semi = cookieValue.indexOf(';', start);
            if (semi < 0) semi = len;
            int pairStart = start;
            while (pairStart < semi && cookieValue.charAt(pairStart) <= ' ') pairStart++;
            int pairEnd = semi;
            while (pairEnd > pairStart && cookieValue.charAt(pairEnd - 1) <= ' ') pairEnd--;
            if (pairEnd > pairStart) {
                h2.add("cookie", cookieValue.substring(pairStart, pairEnd));
            }
            start = semi + 1;
        }
    }

    /**
     * RFC 9113 Section 8.2.3 applied to HTTP/3 (same cookie splitting logic).
     */
    private static void splitCookieToH3(String cookieValue, Http3Headers h3) {
        if (cookieValue.indexOf(';') < 0) {
            h3.add("cookie", cookieValue.trim());
            return;
        }
        int start = 0;
        int len = cookieValue.length();
        while (start < len) {
            int semi = cookieValue.indexOf(';', start);
            if (semi < 0) semi = len;
            int pairStart = start;
            while (pairStart < semi && cookieValue.charAt(pairStart) <= ' ') pairStart++;
            int pairEnd = semi;
            while (pairEnd > pairStart && cookieValue.charAt(pairEnd - 1) <= ' ') pairEnd--;
            if (pairEnd > pairStart) {
                h3.add("cookie", cookieValue.substring(pairStart, pairEnd));
            }
            start = semi + 1;
        }
    }

    /**
     * RFC 9113 Section 8.2.3: When translating HTTP/2 to HTTP/1.1, multiple
     * cookie header entries MUST be concatenated into a single Cookie header
     * using "; " as separator.
     */
    private static void mergeCookiesForH1(Http2Headers h2, HttpHeaders h1) {
        List<CharSequence> cookies = h2.getAll("cookie");
        if (cookies == null || cookies.isEmpty()) {
            return;
        }
        if (cookies.size() == 1) {
            h1.set("cookie", cookies.getFirst().toString());
            return;
        }
        StringBuilder merged = new StringBuilder();
        for (int i = 0; i < cookies.size(); i++) {
            if (i > 0) merged.append("; ");
            merged.append(cookies.get(i));
        }
        h1.set("cookie", merged.toString());
    }

    /**
     * Same as {@link #mergeCookiesForH1} but for HTTP/3 source headers.
     */
    private static void mergeCookiesForH1FromH3(Http3Headers h3, HttpHeaders h1) {
        // Http3Headers does not expose getAll, so we iterate manually
        StringBuilder merged = null;
        int count = 0;
        for (Map.Entry<CharSequence, CharSequence> entry : h3) {
            if ("cookie".equalsIgnoreCase(entry.getKey().toString())) {
                if (merged == null) {
                    merged = new StringBuilder();
                } else {
                    merged.append("; ");
                }
                merged.append(entry.getValue());
                count++;
            }
        }
        if (count > 0) {
            h1.set("cookie", merged.toString());
        }
    }
}
