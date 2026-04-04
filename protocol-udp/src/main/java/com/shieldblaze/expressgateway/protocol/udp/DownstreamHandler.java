/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.udp;

import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;
import io.netty.util.ReferenceCountUtil;
import lombok.extern.log4j.Log4j2;

import java.net.InetSocketAddress;

@Log4j2
final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private final Channel channel;
    private final UDPConnection udpConnection;
    private final InetSocketAddress socketAddress;

    DownstreamHandler(Channel channel, InetSocketAddress socketAddress, UDPConnection udpConnection) {
        this.channel = channel;
        this.udpConnection = udpConnection;
        this.socketAddress = socketAddress;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        DatagramPacket packet = (DatagramPacket) msg;
        try {
            // LOW-26: Wrap in try-finally to prevent double-release on synchronous write failure.
            // We retain the content so the new DatagramPacket owns one ref and the original packet
            // owns another. The finally block releases the original packet (its envelope + content ref).
            // If writeAndFlush succeeds, the pipeline releases the new DatagramPacket's content ref.
            // If writeAndFlush fails synchronously, the pipeline still releases the new ref.
            //
            // Use default promise (not voidPromise) so write failures propagate to
            // exceptionCaught rather than being silently swallowed. VoidPromise ignores
            // all errors, making undeliverable response datagrams invisible to monitoring.
            channel.writeAndFlush(new DatagramPacket(packet.content().retain(), socketAddress))
                    .addListener(ChannelFutureListener.FIRE_EXCEPTION_ON_FAILURE);
        } finally {
            ReferenceCountUtil.safeRelease(packet);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (log.isDebugEnabled()) {
            log.debug("Closing Upstream {} and Downstream {} Channel",
                    socketAddress.getAddress().getHostAddress() + ':' + socketAddress.getPort(),
                    udpConnection.socketAddress().getAddress().getHostAddress() + ':' + udpConnection.socketAddress().getPort());
        }

        udpConnection.close();
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        log.error("Caught Error at Downstream Handler", cause);
        udpConnection.close();
        ctx.channel().close();
    }
}
