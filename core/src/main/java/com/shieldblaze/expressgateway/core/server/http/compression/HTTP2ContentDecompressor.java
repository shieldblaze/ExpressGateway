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

import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.embedded.EmbeddedChannel;
import com.shieldblaze.expressgateway.core.server.http.compression.brotli.BrotliDecoder;
import io.netty.handler.codec.compression.ZlibCodecFactory;
import io.netty.handler.codec.compression.ZlibWrapper;
import io.netty.handler.codec.http2.DelegatingDecompressorFrameListener;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2FrameListener;
import io.netty.handler.codec.http2.Http2Headers;

/**
 * {@link HTTP2ContentCompressor} compresses {@link Http2DataFrame} if {@link Http2Headers} contains {@code Content-Encoding}
 * and is set to:
 * <ul>
 *     <li> gzip </li>
 *     <li> x-gzip </li>
 *     <li> deflate </li>
 *     <li> x-deflate </li>
 *     <li> br </li>
 * </ul>
 */
public class HTTP2ContentDecompressor extends DelegatingDecompressorFrameListener {

    public HTTP2ContentDecompressor(Http2Connection connection, Http2FrameListener listener) {
        super(connection, listener);
    }

    @Override
    protected EmbeddedChannel newContentDecompressor(ChannelHandlerContext ctx, CharSequence contentEncoding) {
        Channel channel = ctx.channel();
        switch (contentEncoding.toString().toLowerCase()) {
            case "gzip":
            case "x-gzip":
                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(), ZlibCodecFactory.newZlibDecoder(ZlibWrapper.GZIP));
            case "deflate":
            case "x-deflate":
                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(), ZlibCodecFactory.newZlibDecoder(ZlibWrapper.ZLIB));
//            case "br":
//                return new EmbeddedChannel(channel.id(), channel.metadata().hasDisconnect(), channel.config(), new BrotliDecoder());
            default:
                return null;
        }
    }
}
