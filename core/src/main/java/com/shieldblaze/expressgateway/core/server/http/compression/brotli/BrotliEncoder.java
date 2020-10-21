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
import com.aayushatharva.brotli4j.encoder.BrotliOutputStream;
import com.aayushatharva.brotli4j.encoder.Encoder;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufOutputStream;
import io.netty.buffer.ByteBufUtil;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.MessageToByteEncoder;
import io.netty.util.internal.logging.InternalLogger;
import io.netty.util.internal.logging.InternalLoggerFactory;

/**
 * Brotli Encoder (Compressor)
 */
public class BrotliEncoder extends MessageToByteEncoder<ByteBuf> {

    private static final InternalLogger logger = InternalLoggerFactory.getInstance(BrotliEncoder.class);

    static {
        logger.info("Brotli4j Loader Status: {}", Brotli4jLoader.isAvailable());
    }

    private final Encoder.Parameters parameters;
    private BrotliOutputStream brotliOutputStream;
    private ByteBuf byteBuf;

    /**
     * Create new {@link BrotliEncoder} Instance
     *
     * @param quality Quality of Encoding (Compression)
     */
    public BrotliEncoder(int quality) {
        this.parameters = new Encoder.Parameters().setQuality(quality);
    }

    @Override
    public void encode(ChannelHandlerContext ctx, ByteBuf msg, ByteBuf out) throws Exception {
        if (msg.readableBytes() == 0) {
            out.writeBytes(msg);
            return;
        }

        if (brotliOutputStream == null) {
            byteBuf = ctx.alloc().buffer();
            brotliOutputStream = new BrotliOutputStream(new ByteBufOutputStream(byteBuf), parameters);
        }

        if (msg.hasArray()) {
            byte[] inAry = msg.array();
            int offset = msg.arrayOffset() + msg.readerIndex();
            int len = msg.readableBytes();
            brotliOutputStream.write(inAry, offset, len);
        } else {
            brotliOutputStream.write(ByteBufUtil.getBytes(msg));
        }

        brotliOutputStream.flush();
        out.writeBytes(byteBuf);
        byteBuf.clear();

        if (!out.isWritable()) {
            out.ensureWritable(out.writerIndex());
        }
    }

    @Override
    public void close(ChannelHandlerContext ctx, ChannelPromise promise) throws Exception {
        if (brotliOutputStream != null) {
            brotliOutputStream.close();
            byteBuf.release();
            promise.setSuccess();
            brotliOutputStream = null;
        }
    }
}
