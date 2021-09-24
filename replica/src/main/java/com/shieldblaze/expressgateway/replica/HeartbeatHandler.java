package com.shieldblaze.expressgateway.replica;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;

/**
 * This class handles Heartbeat service.
 */
@ChannelHandler.Sharable
public final class HeartbeatHandler extends ChannelInboundHandlerAdapter implements Runnable {

    public static final HeartbeatHandler INSTANCE = new HeartbeatHandler();
    private static final ByteBuf HEARTBEAT = Unpooled
            .buffer()
            .writeBytes("HeartBeatV1".getBytes(StandardCharsets.UTF_8));

    private final ScheduledExecutorService ses = Executors.newSingleThreadScheduledExecutor();
    private Future<?> future;
    private Channel channel;

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf buf = (ByteBuf) msg;

        // Check if message is HeartBeat. If it is then write back
        // the heartbeat message.
        if (ByteBufUtil.equals(buf, HEARTBEAT)) {
            ctx.writeAndFlush(buf);
        } else {
            ctx.fireChannelRead(msg);
        }
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        channel = ctx.channel();
        future = ses.scheduleWithFixedDelay(this, 0, 2, TimeUnit.SECONDS);
        super.channelActive(ctx);
    }

    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) throws Exception {
        future.cancel(true);
        ses.shutdownNow();
        super.handlerRemoved(ctx);
    }

    @Override
    public void run() {
        channel.writeAndFlush(HEARTBEAT.retainedDuplicate());
    }

    private HeartbeatHandler() {
        // Prevent outside initialization
    }
}
