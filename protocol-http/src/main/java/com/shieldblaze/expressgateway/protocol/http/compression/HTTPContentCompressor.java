/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.protocol.http.compression.brotli.BrotliEncoder;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.compression.ZlibCodecFactory;
import io.netty.handler.codec.compression.ZlibWrapper;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpResponse;

/**
 * {@link HTTPContentCompressor} compresses {@link HttpContent} of {@link HttpResponse} if
 * {@link HttpHeaderNames#CONTENT_TYPE} is compressible.
 */
public class HTTPContentCompressor extends HttpContentCompressor {

    private final int brotliCompressionQuality;
    private final int compressionLevel;

    private ChannelHandlerContext ctx;

    public HTTPContentCompressor(HTTPConfiguration httpConfiguration) {
        this.brotliCompressionQuality = httpConfiguration.brotliCompressionLevel();
        this.compressionLevel = httpConfiguration.deflateCompressionLevel();
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    @Override
    protected Result beginEncode(HttpResponse response, String acceptEncoding) {
        String targetContentEncoding = HTTPCompressionUtil.targetEncoding(response, acceptEncoding);

        if (targetContentEncoding == null) {
            return null;
        }

        ChannelHandler compressor;
        switch (targetContentEncoding) {
            case "gzip":
                compressor = ZlibCodecFactory.newZlibEncoder(ZlibWrapper.GZIP, compressionLevel, 15, 8);
                break;
            case "deflate":
                compressor = ZlibCodecFactory.newZlibEncoder(ZlibWrapper.ZLIB, compressionLevel, 15, 8);
                break;
            case "br":
                compressor = new BrotliEncoder(brotliCompressionQuality);
                break;
            default:
                throw new Error();
        }

        return new Result(targetContentEncoding, new EmbeddedChannel(ctx.channel().id(), ctx.channel().metadata().hasDisconnect(), ctx.channel().config(), compressor));
    }
}
