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
package com.shieldblaze.expressgateway.backend;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufHolder;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

/**
 * Handler which tracks the count of Bytes IN/OUT.
 * This handler must be the first handler in Pipeline.
 */
public final class NodeBytesTracker extends ChannelDuplexHandler {

    private final Node node;

    @NonNull
    public NodeBytesTracker(Node node) {
        this.node = node;
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof ByteBuf) {
            int bytes = ((ByteBuf) msg).readableBytes();
            node.incBytesSent(bytes);
        } else if (msg instanceof ByteBufHolder) {
            int bytes = ((ByteBufHolder) msg).content().readableBytes();
            node.incBytesSent(bytes);
        }
        super.write(ctx, msg, promise);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof ByteBuf) {
            int bytes = ((ByteBuf) msg).readableBytes();
            node.incBytesReceived(bytes);
        } else if (msg instanceof ByteBufHolder) {
            int bytes = ((ByteBufHolder) msg).content().readableBytes();
            node.incBytesReceived(bytes);
        }
        super.channelRead(ctx, msg);
    }
}
