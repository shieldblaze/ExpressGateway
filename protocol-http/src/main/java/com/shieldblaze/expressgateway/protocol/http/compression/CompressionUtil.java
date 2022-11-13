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
import java.util.TreeSet;

import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_ENCODING;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_TYPE;

public final class CompressionUtil {
    private static final boolean EnableBrotli = true;
    private static final boolean EnableGzip = true;
    private static final boolean EnableDeflate = true;
    private static final boolean EnableZstd = true;

    private static final Set<String> MIME_TYPES = new TreeSet<>();

    static {
        MIME_TYPES.add("text/html");
        MIME_TYPES.add("text/css");
        MIME_TYPES.add("text/plain");
        MIME_TYPES.add("text/xml");
        MIME_TYPES.add("text/x-component");
        MIME_TYPES.add("text/javascript");
        MIME_TYPES.add("application/x-javascript");
        MIME_TYPES.add("application/javascript");
        MIME_TYPES.add("application/json");
        MIME_TYPES.add("application/manifest+json");
        MIME_TYPES.add("application/xml");
        MIME_TYPES.add("application/xhtml+xml");
        MIME_TYPES.add("application/rss+xml");
        MIME_TYPES.add("application/atom+xml");
        MIME_TYPES.add("application/vnd.ms-fontobject");
        MIME_TYPES.add("application/x-font-ttf");
        MIME_TYPES.add("application/x-font-opentype");
        MIME_TYPES.add("application/x-font-truetype");
        MIME_TYPES.add("image/svg+xml");
        MIME_TYPES.add("image/x-icon");
        MIME_TYPES.add("image/vnd.microsoft.icon");
        MIME_TYPES.add("font/ttf");
        MIME_TYPES.add("font/eot");
        MIME_TYPES.add("font/otf");
        MIME_TYPES.add("font/opentype");
    }

    public static boolean isCompressible(String contentType) {
        return MIME_TYPES.contains(contentType.split(";")[0]);
    }

    public static String checkCompressibleForHttp2(Http2Headers headers, String acceptEncoding, long compressionThreshold) {
        if (headers.contains(CONTENT_ENCODING)) {
            return null;
        } else if (acceptEncoding == null) {
            return null;
        } else if (!headers.contains(CONTENT_TYPE)) {
            return null;
        } else if (!MIME_TYPES.contains(headers.get(CONTENT_TYPE).toString().split(";")[0])) {
            return null;
        } else if (headers.contains(CONTENT_LENGTH)) {
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
            float q = 1.0f;
            int equalsPos = encoding.indexOf('=');
            if (equalsPos != -1) {
                try {
                    q = Float.parseFloat(encoding.substring(equalsPos + 1));
                } catch (NumberFormatException e) {
                    // Ignore encoding
                    q = 0.0f;
                }
            }
            if (encoding.contains("*")) {
                starQ = q;
            } else if (encoding.contains("br") && q > brQ) {
                brQ = q;
            } else if (encoding.contains("zstd") && q > zstdQ) {
                zstdQ = q;
            } else if (encoding.contains("gzip") && q > gzipQ) {
                gzipQ = q;
            } else if (encoding.contains("deflate") && q > deflateQ) {
                deflateQ = q;
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
}
