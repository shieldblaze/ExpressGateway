package com.shieldblaze.expressgateway.replica;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;

/**
 * This class takes care of handling Member messages.
 */
public class MemberHandler extends SimpleChannelInboundHandler<ByteBuf> {

    private ChannelHandlerContext ctx;

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    @Override
    protected void channelRead0(ChannelHandlerContext ctx, ByteBuf msg) {

    }

    public void sendAnnouncement(ByteBuf buf) {
        ctx.writeAndFlush(buf);
    }
}
