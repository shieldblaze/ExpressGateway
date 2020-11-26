/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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

import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http2.Http2Headers;

import java.util.Set;
import java.util.TreeSet;

public final class HTTPCompressionUtil {

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

    public static String targetEncoding(Http2Headers headers, String acceptEncoding) {
        // If "CONTENT-ENCODING" is already set then we will not do anything.
        if (headers.contains(HttpHeaderNames.CONTENT_ENCODING)) {
            return null;
        }

        if (!headers.contains(HttpHeaderNames.CONTENT_TYPE)) {
            return null;
        }

        if (!MIME_TYPES.contains(headers.get(HttpHeaderNames.CONTENT_TYPE).toString().split(";")[0])) {
            return null;
        }

        return determineEncoding(acceptEncoding);
    }

    public static String targetEncoding(HttpResponse response, String acceptEncoding) {

        // If "CONTENT-ENCODING" is already set then we will not do anything.
        if (response.headers().contains(HttpHeaderNames.CONTENT_ENCODING)) {
            return null;
        }

        if (!response.headers().contains(HttpHeaderNames.CONTENT_TYPE)) {
            return null;
        }

        if (!MIME_TYPES.contains(response.headers().get(HttpHeaderNames.CONTENT_TYPE).split(";")[0])) {
            return null;
        }

        return determineEncoding(acceptEncoding);
    }

    private static String determineEncoding(String acceptEncoding) {
        float starQ = -1.0f;
        float brQ = -1.0f;
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
            } else if (encoding.contains("gzip") && q > gzipQ) {
                gzipQ = q;
            } else if (encoding.contains("deflate") && q > deflateQ) {
                deflateQ = q;
            }
        }
        if (brQ > 0.0f || gzipQ > 0.0f || deflateQ > 0.0f) {
            if (brQ >= gzipQ) {
                return "br";
            } else if (gzipQ >= deflateQ) {
                return "gzip";
            } else {
                return "deflate";
            }
        }
        if (starQ > 0.0f) {
            if (brQ == -1.0f) {
                return "br";
            }
            if (gzipQ == -1.0f) {
                return "gzip";
            }
            if (deflateQ == -1.0f) {
                return "deflate";
            }
        }
        return null;
    }
}
