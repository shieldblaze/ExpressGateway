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
package com.shieldblaze.expressgateway.server.tcp;

import com.shieldblaze.expressgateway.netty.BootstrapUtils;
import com.shieldblaze.expressgateway.netty.EventLoopFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.util.ReferenceCounted;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * <p> Upstream Handler receives Data from Internet.
 * This is the first point of contact for Load Balancer. </p>
 *
 * <p> Flow: </p>
 * <p> &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp; &nbsp;
 * &nbsp; &nbsp; &nbsp; (Data) </p>
 * (INTERNET) -->-->-->--> (EXPRESSGATEWAY) -->-->-->--> (BACKEND)
 */
final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private ConcurrentLinkedQueue<ByteBuf> backlog = new ConcurrentLinkedQueue<>();
    private boolean channelActive = false;
    private Channel backendChannel;

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        Channel sourceChannel = ctx.channel();
        Bootstrap bootstrap = BootstrapUtils.tcp(ctx.channel().eventLoop());
        bootstrap.handler(new DownstreamHandler(sourceChannel));

        ChannelFuture channelFuture = bootstrap.connect(new InetSocketAddress("127.0.0.1", 9111));
        backendChannel = channelFuture.channel();

        // Listener for writing Backlog
        channelFuture.addListener((ChannelFutureListener) future -> {

            /*
             * If we're connected to the backend, then we'll send all backlog to backend.
             *
             * If we're not connected to the backend, close everything.
             */
            if (future.isSuccess()) {
                EventLoopFactory.CHILD.next().execute(() -> {

                    backlog.forEach(packet -> {
                        backendChannel.writeAndFlush(packet).addListener((ChannelFutureListener) cf -> {
                            if (!cf.isSuccess()) {
                                packet.release();
                            }
                        });
                    });

                    channelActive = true;
                    backlog = null;
                });
            } else {
                backlog.forEach(ReferenceCounted::release);
                backlog.clear();
                backendChannel.close();
                ctx.channel().close();
            }
        });
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (channelActive) {
            backendChannel.writeAndFlush(msg);
        } else if (backlog != null) {
            backlog.add((ByteBuf) msg);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        ctx.channel().close();
        backendChannel.close();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
    }
}
