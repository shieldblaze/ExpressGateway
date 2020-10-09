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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.compression.BrotliEncoder;
import io.netty.handler.codec.compression.ZlibCodecFactory;
import io.netty.handler.codec.compression.ZlibWrapper;
import io.netty.handler.codec.http2.CompressorHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;

/**
 * {@link HTTP2ContentDecompressor} decompresses {@link Http2DataFrame} if {@link Http2Headers} contains {@code Content-Encoding}
 * and is set to:
 * <ul>
 *     <li> gzip </li>
 *     <li> x-gzip </li>
 *     <li> deflate </li>
 *     <li> x-deflate </li>
 *     <li> br </li>
 * </ul>
 */
final class HTTP2ContentCompressor extends CompressorHttp2ConnectionEncoder {

    private final int brotliCompressionQuality;
    private final int compressionLevel;

    HTTP2ContentCompressor(Http2ConnectionEncoder delegate, HTTPConfiguration httpConfiguration) {
        super(delegate);
        this.brotliCompressionQuality = httpConfiguration.getBrotliCompressionLevel();
        this.compressionLevel = httpConfiguration.getDeflateCompressionLevel();
    }

    @Override
    protected EmbeddedChannel newContentCompressor(ChannelHandlerContext ctx, CharSequence contentEncoding) {
        Channel channel = ctx.channel();
        switch (contentEncoding.toString().toLowerCase()) {
            case "gzip":
            case "x-gzip":
                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(),
                        ZlibCodecFactory.newZlibEncoder(ZlibWrapper.GZIP, compressionLevel, 15, 8));
            case "deflate":
            case "x-deflate":
                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(),
                        ZlibCodecFactory.newZlibEncoder(ZlibWrapper.ZLIB, compressionLevel, 15, 8));
            case "br":
                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(), new BrotliEncoder(brotliCompressionQuality));
            default:
                return null;
        }
    }
}
