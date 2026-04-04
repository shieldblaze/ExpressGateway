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
package com.shieldblaze.expressgateway.protocol.http.compression;

import io.netty.handler.codec.http2.Http2Headers;

import java.util.Set;

import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_ENCODING;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_TYPE;

public final class CompressionUtil {

    private static final boolean EnableBrotli = true;
    private static final boolean EnableGzip = true;
    private static final boolean EnableDeflate = true;
    private static final boolean EnableZstd = true;

    /**
     * Immutable set of compressible MIME types. Uses {@code Set.of()} for O(1) hash-based
     * lookups instead of {@code TreeSet}'s O(log n) tree traversal. The set is unmodifiable
     * and internally optimized by the JDK for small-to-medium cardinalities.
     */
    private static final Set<String> MIME_TYPES = Set.of(
            "text/html",
            "text/css",
            "text/plain",
            "text/xml",
            "text/x-component",
            "text/javascript",
            "application/x-javascript",
            "application/javascript",
            "application/json",
            "application/manifest+json",
            "application/xml",
            "application/xhtml+xml",
            "application/rss+xml",
            "application/atom+xml",
            "application/vnd.ms-fontobject",
            "application/x-font-ttf",
            "application/x-font-opentype",
            "application/x-font-truetype",
            "application/wasm",
            "image/svg+xml",
            "image/x-icon",
            "image/vnd.microsoft.icon",
            "font/ttf",
            "font/eot",
            "font/otf",
            "font/opentype"
    );

    public static boolean isCompressible(String contentType) {
        // Avoid split(";") allocation on hot path — find the semicolon index manually.
        int semicolon = contentType.indexOf(';');
        String mimeType = semicolon >= 0 ? contentType.substring(0, semicolon) : contentType;
        return MIME_TYPES.contains(mimeType);
    }

    public static String checkCompressibleForHttp2(Http2Headers headers, String acceptEncoding, long compressionThreshold) {
        if (headers.contains(CONTENT_ENCODING)) {
            return null;
        }
        if (acceptEncoding == null) {
            return null;
        }
        if (!headers.contains(CONTENT_TYPE)) {
            return null;
        }
        String ct = headers.get(CONTENT_TYPE).toString();
        int semi = ct.indexOf(';');
        if (!MIME_TYPES.contains(semi >= 0 ? ct.substring(0, semi) : ct)) {
            return null;
        }
        if (headers.contains(CONTENT_LENGTH)) {
            if (!(headers.getLong(CONTENT_LENGTH, -1) >= compressionThreshold)) {
                return null;
            }
        }

        return determineEncoding(acceptEncoding);
    }

    @SuppressWarnings("FloatingPointEquality")
    private static String determineEncoding(String acceptEncoding) {
        float starQ = -1.0f;
        float brQ = -1.0f;
        float zstdQ = -1.0f;
        float gzipQ = -1.0f;
        float deflateQ = -1.0f;
        for (String encoding : acceptEncoding.split(",")) {
            // Parse "encoding;q=value" — extract the token before ';' and the q-value after '='.
            String token;
            float q = 1.0f;
            int semiPos = encoding.indexOf(';');
            if (semiPos != -1) {
                token = encoding.substring(0, semiPos).trim().toLowerCase();
                int equalsPos = encoding.indexOf('=', semiPos);
                if (equalsPos != -1) {
                    try {
                        q = Float.parseFloat(encoding.substring(equalsPos + 1).trim());
                    } catch (NumberFormatException e) {
                        q = 0.0f;
                    }
                }
            } else {
                token = encoding.trim().toLowerCase();
            }
            // Match exact encoding tokens to prevent false positives
            // (e.g., "br" must not match "x-br-custom").
            switch (token) {
                case "*" -> starQ = q;
                case "br" -> { if (q > brQ) brQ = q; }
                case "zstd" -> { if (q > zstdQ) zstdQ = q; }
                case "gzip" -> { if (q > gzipQ) gzipQ = q; }
                case "deflate" -> { if (q > deflateQ) deflateQ = q; }
                default -> { /* unsupported encoding, ignore */ }
            }
        }
        if (brQ > 0.0f || zstdQ > 0.0f || gzipQ > 0.0f || deflateQ > 0.0f) {
            if (brQ != -1.0f && brQ >= zstdQ && EnableBrotli) {
                return "br";
            } else if (zstdQ != -1.0f && zstdQ >= gzipQ && EnableZstd) {
                return "zstd";
            } else if (gzipQ != -1.0f && gzipQ >= deflateQ && EnableGzip) {
                return "gzip";
            } else if (deflateQ != -1.0f && EnableDeflate) {
                return "deflate";
            }
        }
        if (starQ > 0.0f) {
            if (brQ == -1.0f && EnableBrotli) {
                return "br";
            }
            if (zstdQ == -1.0f && EnableZstd) {
                return "zstd";
            }
            if (gzipQ == -1.0f && EnableGzip) {
                return "gzip";
            }
            if (deflateQ == -1.0f && EnableDeflate) {
                return "deflate";
            }
        }
        return null;
    }

    private CompressionUtil() {
        // Prevent outside initialization
    }
}
