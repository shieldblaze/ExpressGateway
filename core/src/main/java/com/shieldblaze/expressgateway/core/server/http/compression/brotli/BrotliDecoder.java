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
package com.shieldblaze.expressgateway.core.server.http.compression.brotli;

import com.aayushatharva.brotli4j.Brotli4jLoader;
import com.aayushatharva.brotli4j.decoder.Decoder;
import com.aayushatharva.brotli4j.decoder.DecoderJNI;
import com.aayushatharva.brotli4j.decoder.DirectDecompress;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.ByteToMessageDecoder;
import io.netty.util.internal.logging.InternalLogger;
import io.netty.util.internal.logging.InternalLoggerFactory;

import java.util.List;

/**
 * Brotli Decoder (Decompressor)
 */
public class BrotliDecoder extends ByteToMessageDecoder {

    private static final InternalLogger logger = InternalLoggerFactory.getInstance(BrotliDecoder.class);

    static {
        logger.info("Brotli4j Loader Status: {}", Brotli4jLoader.isAvailable());
    }

    /**
     * Aggregate up to a single block
     */
    private ByteBuf aggregatingBuf;

    @Override
    protected void decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out) throws Exception {
        if (in.readableBytes() == 0) {
            out.add(in);
            return;
        }

        if (aggregatingBuf == null) {
            aggregatingBuf = ctx.alloc().buffer();
        }

        aggregatingBuf.writeBytes(in);

        byte[] compressedData = ByteBufUtil.getBytes(aggregatingBuf, aggregatingBuf.readerIndex(), aggregatingBuf.readableBytes(), false);
        DirectDecompress directDecompress = Decoder.decompress(compressedData);

        if (directDecompress.getResultStatus() == DecoderJNI.Status.DONE) {
            aggregatingBuf.clear();
            out.add(ctx.alloc().buffer().writeBytes(directDecompress.getDecompressedData()));
        }
    }

    @Override
    protected void handlerRemoved0(ChannelHandlerContext ctx) {
        if (aggregatingBuf != null) {
            if (aggregatingBuf.refCnt() > 0) {
                aggregatingBuf.release();
            }
            aggregatingBuf = null;
        }
    }
}
