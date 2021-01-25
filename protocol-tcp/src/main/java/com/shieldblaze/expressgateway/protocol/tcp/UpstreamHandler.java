/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private final L4LoadBalancer l4LoadBalancer;
    private final Bootstrapper bootstrapper;
    private TCPConnection tcpConnection;

    UpstreamHandler(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        bootstrapper = new Bootstrapper(l4LoadBalancer, l4LoadBalancer.eventLoopFactory().childGroup(), l4LoadBalancer.byteBufAllocator());
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        Node node;
        try {
            node = l4LoadBalancer.cluster("DEFAULT").nextNode(new L4Request((InetSocketAddress) ctx.channel().remoteAddress())).node();
            tcpConnection = bootstrapper.newInit(node, ctx.channel());
            node.addConnection(tcpConnection);
        } catch (Exception ex) {
            ctx.close();
            throw ex;
        } finally {
            if (tcpConnection == null) {
                ctx.close();
            }
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf byteBuf = (ByteBuf) msg;
        tcpConnection.node().incBytesSent(byteBuf.readableBytes());
        tcpConnection.writeAndFlush(msg);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            InetSocketAddress socketAddress = ((InetSocketAddress) ctx.channel().remoteAddress());
            if (tcpConnection == null || tcpConnection.socketAddress() == null) {
                logger.info("Closing Upstream {}",
                        socketAddress.getAddress().getHostAddress() + ":" + socketAddress.getPort());
            } else {
                logger.info("Closing Upstream {} and Downstream {} Channel",
                        socketAddress.getAddress().getHostAddress() + ":" + socketAddress.getPort(),
                        tcpConnection.socketAddress().getAddress().getHostAddress() + ":" + tcpConnection.socketAddress().getPort());
            }
        }

        if (ctx.channel().isActive()) {
            ctx.channel().close();
        }

        if (tcpConnection != null && tcpConnection.isActive()) {
            tcpConnection.close();
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
