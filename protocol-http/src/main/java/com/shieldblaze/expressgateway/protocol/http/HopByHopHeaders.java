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

import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.Http2Headers;

import java.util.List;
import java.util.Set;

/**
 * Strips hop-by-hop headers from HTTP messages per RFC 7230 Section 6.1.
 *
 * <p>A proxy MUST remove hop-by-hop headers before forwarding a message.
 * The following headers are hop-by-hop by definition:
 * <ul>
 *   <li>Connection</li>
 *   <li>Keep-Alive</li>
 *   <li>Proxy-Authenticate</li>
 *   <li>Proxy-Authorization</li>
 *   <li>TE</li>
 *   <li>Trailers</li>
 *   <li>Transfer-Encoding</li>
 *   <li>Upgrade (except when performing a WebSocket upgrade)</li>
 * </ul>
 *
 * <p>Additionally, any headers listed in the {@code Connection} header value
 * are also hop-by-hop and MUST be removed (RFC 7230 Section 6.1).
 *
 * <p>Thread-safety: All methods are stateless and safe for concurrent use.
 */
public final class HopByHopHeaders {

    // Canonical set of hop-by-hop header names (lowercase for case-insensitive comparison).
    // "upgrade" is handled separately to allow WebSocket upgrades through.
    private static final Set<String> HOP_BY_HOP_NAMES = Set.of(
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailers",
            "transfer-encoding",
            "upgrade"
    );

    private HopByHopHeaders() {
        // Prevent outside initialization
    }

    /**
     * Strips hop-by-hop headers from HTTP/1.1 {@link HttpHeaders}.
     *
     * <p>Per RFC 7230 Section 6.1, headers listed in the {@code Connection} header
     * value are also removed. The {@code Upgrade} header is preserved when the
     * request is a WebSocket upgrade (Connection: Upgrade, Upgrade: websocket).
     *
     * <p>{@code Transfer-Encoding} is stripped on the request path because the proxy
     * re-encodes for the backend. On the response path, use {@link #stripForResponse(HttpHeaders)}
     * which preserves {@code Transfer-Encoding} so Netty's codec can handle chunked encoding.
     *
     * @param headers the HTTP/1.1 headers to strip; modified in place
     */
    public static void strip(HttpHeaders headers) {
        stripInternal(headers, true);
    }

    /**
     * Strips hop-by-hop headers from HTTP/1.1 response headers, preserving
     * {@code Transfer-Encoding} which Netty's {@code HttpServerCodec} needs
     * for correct response encoding on the client-facing connection.
     *
     * @param headers the HTTP/1.1 response headers to strip; modified in place
     */
    public static void stripForResponse(HttpHeaders headers) {
        stripInternal(headers, false);
    }

    private static void stripInternal(HttpHeaders headers, boolean stripTransferEncoding) {
        if (headers == null || headers.isEmpty()) {
            return;
        }

        boolean isWebSocketUpgrade = isWebSocketUpgrade(headers);

        // GC-03: Per RFC 7230 Section 6.1: the Connection header field itself lists
        // additional hop-by-hop headers that the sender wants removed. Parse them
        // before we remove the Connection header.
        //
        // Optimized to avoid getAll() (ArrayList allocation) and split(",") (String[]
        // allocation). Most requests have 0 or 1 Connection header with a single
        // token (e.g., "keep-alive"), so we optimize for that case with manual
        // comma scanning.
        // Process ALL Connection header values (there may be multiple headers).
        // Use getAll() to ensure correctness, then apply zero-alloc comma parsing.
        java.util.List<String> connectionValues = headers.getAll(HttpHeaderNames.CONNECTION);
        if (connectionValues != null) {
            for (String connectionValue : connectionValues) {
                parseAndRemoveConnectionTokens(headers, connectionValue);
            }
        }

        // Remove the standard hop-by-hop headers
        headers.remove(HttpHeaderNames.CONNECTION);
        headers.remove("keep-alive");
        headers.remove(HttpHeaderNames.PROXY_AUTHENTICATE);
        headers.remove(HttpHeaderNames.PROXY_AUTHORIZATION);
        headers.remove(HttpHeaderNames.TE);
        headers.remove("trailers");
        if (stripTransferEncoding) {
            headers.remove(HttpHeaderNames.TRANSFER_ENCODING);
        }

        // Only strip Upgrade if this is NOT a WebSocket upgrade
        if (!isWebSocketUpgrade) {
            headers.remove(HttpHeaderNames.UPGRADE);
        }
    }

    /**
     * Strips hop-by-hop headers from HTTP/2 {@link Http2Headers}.
     *
     * <p>HTTP/2 does not use hop-by-hop headers in the same way as HTTP/1.1
     * (RFC 9113 Section 8.2.2 explicitly forbids Connection-specific headers).
     * However, when converting between HTTP/1.1 and HTTP/2 at the proxy layer,
     * remnants of these headers may be present and must be cleaned.
     *
     * @param headers the HTTP/2 headers to strip; modified in place
     */
    public static void strip(Http2Headers headers) {
        if (headers == null) {
            return;
        }

        // GC-03: Parse Connection header tokens before removing it, same as HTTP/1.1.
        // Use manual comma scanning to avoid toString().split(",") allocations.
        CharSequence connectionCs = headers.get("connection");
        if (connectionCs != null) {
            String connectionValue = connectionCs.toString();
            if (connectionValue.indexOf(',') < 0) {
                // Fast path: single token, no comma
                String trimmed = connectionValue.trim();
                if (!trimmed.isEmpty() && !"upgrade".equalsIgnoreCase(trimmed)) {
                    headers.remove(trimmed);
                }
            } else {
                // Slow path: comma-separated tokens
                int start = 0;
                int len = connectionValue.length();
                while (start < len) {
                    int comma = connectionValue.indexOf(',', start);
                    if (comma < 0) comma = len;
                    int tokenStart = start;
                    while (tokenStart < comma && connectionValue.charAt(tokenStart) <= ' ') tokenStart++;
                    int tokenEnd = comma;
                    while (tokenEnd > tokenStart && connectionValue.charAt(tokenEnd - 1) <= ' ') tokenEnd--;
                    if (tokenEnd > tokenStart) {
                        String name = connectionValue.substring(tokenStart, tokenEnd);
                        if (!"upgrade".equalsIgnoreCase(name)) {
                            headers.remove(name);
                        }
                    }
                    start = comma + 1;
                }
            }
        }

        headers.remove("connection");
        headers.remove("keep-alive");
        headers.remove("proxy-authenticate");
        headers.remove("proxy-authorization");
        headers.remove("te");
        headers.remove("trailers");
        // Transfer-Encoding is not meaningful in HTTP/2 (RFC 9113 Section 8.2.2),
        // so always strip it regardless of request/response direction.
        headers.remove("transfer-encoding");
        headers.remove("upgrade");
    }

    /**
     * Checks if the given HTTP/1.1 headers represent a WebSocket upgrade request.
     *
     * @param headers the HTTP/1.1 headers to inspect
     * @return {@code true} if this is a WebSocket upgrade
     */
    /**
     * GC-03: Parse a Connection header value for comma-separated tokens and remove
     * the named headers. Uses manual comma scanning to avoid split() allocations.
     */
    private static void parseAndRemoveConnectionTokens(HttpHeaders headers, String connectionValue) {
        if (connectionValue.indexOf(',') < 0) {
            // Fast path: single token, no comma
            String trimmed = connectionValue.trim();
            if (!trimmed.isEmpty() && !"upgrade".equalsIgnoreCase(trimmed)) {
                headers.remove(trimmed);
            }
        } else {
            // Slow path: comma-separated tokens
            int start = 0;
            int len = connectionValue.length();
            while (start < len) {
                int comma = connectionValue.indexOf(',', start);
                if (comma < 0) comma = len;
                int tokenStart = start;
                while (tokenStart < comma && connectionValue.charAt(tokenStart) <= ' ') tokenStart++;
                int tokenEnd = comma;
                while (tokenEnd > tokenStart && connectionValue.charAt(tokenEnd - 1) <= ' ') tokenEnd--;
                if (tokenEnd > tokenStart) {
                    String name = connectionValue.substring(tokenStart, tokenEnd);
                    if (!"upgrade".equalsIgnoreCase(name)) {
                        headers.remove(name);
                    }
                }
                start = comma + 1;
            }
        }
    }

    private static boolean isWebSocketUpgrade(HttpHeaders headers) {
        String connection = headers.get(HttpHeaderNames.CONNECTION);
        String upgrade = headers.get(HttpHeaderNames.UPGRADE);
        return connection != null && upgrade != null
                && connection.toLowerCase().contains("upgrade")
                && "websocket".equalsIgnoreCase(upgrade);
    }
}
