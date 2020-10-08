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
package com.shieldblaze.expressgateway.core.server.http.compression;

import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.compression.BrotliEncoder;
import io.netty.handler.codec.compression.ZlibCodecFactory;
import io.netty.handler.codec.compression.ZlibWrapper;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpResponse;

import java.util.HashSet;
import java.util.Set;

/**
 * {@link HTTPContentCompressor} compresses {@link HttpContent} of {@link HttpResponse} if
 * {@link HttpHeaderNames#CONTENT_TYPE} is compressible.
 */
public final class HTTPContentCompressor extends HttpContentCompressor {

    private static final Set<String> MIME_TYPES = new HashSet<>();

    private final int brotliCompressionQuality;
    private final int compressionLevel;
    private final int windowBits;
    private final int memLevel;

    private ChannelHandlerContext ctx;

    public HTTPContentCompressor(int brotliCompressionQuality, int compressionLevel, int windowBits, int memLevel) {
        this.brotliCompressionQuality = brotliCompressionQuality;
        this.compressionLevel = compressionLevel;
        this.windowBits = windowBits;
        this.memLevel = memLevel;
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    @Override
    protected Result beginEncode(HttpResponse response, String acceptEncoding) {
        String targetContentEncoding = getTargetEncoding(response, acceptEncoding);

        if (targetContentEncoding == null) {
            return null;
        }

        ChannelHandler compressor;
        switch (targetContentEncoding) {
            case "gzip":
                compressor = ZlibCodecFactory.newZlibEncoder(ZlibWrapper.GZIP, compressionLevel, windowBits, memLevel);
                break;
            case "deflate":
                compressor = ZlibCodecFactory.newZlibEncoder(ZlibWrapper.ZLIB, compressionLevel, windowBits, memLevel);
                break;
            case "br":
                compressor = new BrotliEncoder(brotliCompressionQuality);
                break;
            default:
                throw new Error();
        }

        return new Result(targetContentEncoding, new EmbeddedChannel(ctx.channel().id(), ctx.channel().metadata().hasDisconnect(),
                ctx.channel().config(), compressor));
    }

    static String getTargetEncoding(HttpResponse response, String acceptEncoding) {
        HttpHeaders headers = response.headers();
        // If `Content-Encoding` is set to `Identity`, then we'll do nothing.
        if (headers.containsValue(HttpHeaderNames.CONTENT_ENCODING, HttpHeaderValues.IDENTITY, true)) {
            return null;
        }

        String targetContentEncoding = determineEncoding(acceptEncoding);
        if (targetContentEncoding == null) {
            return null;
        }

        /*
         * If MIME_TYPE is compressible and `Brotli` is selected for compression and Response already contains `CONTENT_ENCODING`
         * then we'll check value is `gzip` or `deflate`.
         *
         * If `true` then we'll recompress it with Brotli for better compression.
         */
        String contentType = headers.get(HttpHeaderNames.CONTENT_TYPE);
        if (targetContentEncoding.equals("br") && MIME_TYPES.contains(contentType) && headers.contains(HttpHeaderNames.CONTENT_ENCODING)) {
            String contentEncoding = headers.get(HttpHeaderNames.CONTENT_ENCODING);
            if (!(contentEncoding.equalsIgnoreCase("gzip") || contentEncoding.equalsIgnoreCase("deflate"))) {
                return null;
            }
        }

        return targetContentEncoding;
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
}
