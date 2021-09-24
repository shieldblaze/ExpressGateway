package com.shieldblaze.expressgateway.replica;

import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;

@ChannelHandler.Sharable
public final class ReplicaHandler extends ChannelInboundHandlerAdapter {

    public static final ReplicaHandler INSTANCE = new ReplicaHandler();

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {

    }

    private ReplicaHandler() {
        // Prevent outside initialization
    }
}
