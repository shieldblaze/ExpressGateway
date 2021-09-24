package com.shieldblaze.expressgateway.replica;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.ByteToMessageDecoder;

import java.nio.charset.StandardCharsets;
import java.util.List;

@ChannelHandler.Sharable
public final class ReplicaDecoder extends ByteToMessageDecoder {

    public static final ReplicaDecoder INSTANCE = new ReplicaDecoder();

    private static final ByteBuf BAD_REQUEST_MESSAGE = Unpooled.buffer()
            .writeBytes("Bad Request".getBytes(StandardCharsets.UTF_8));

    @Override
    protected void decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out) throws Exception {
        short magic = in.readShort();
        if (magic != Messages.MAGIC) {
            writeBadRequest(ctx);
            return;
        }

        switch (in.readByte()) {
            case Messages.MEMBER_ADD: {

            }

            default:
                writeBadRequest(ctx);
                break;
        }
    }

    private void writeBadRequest(ChannelHandlerContext ctx) {
        ctx.writeAndFlush(BAD_REQUEST_MESSAGE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
    }

    private ReplicaDecoder() {
        // Prevent outside initialization
    }
}
