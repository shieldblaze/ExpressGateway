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
package com.shieldblaze.expressgateway.core.server.tcp;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.l4.L4Balance;
import com.shieldblaze.expressgateway.core.netty.BootstrapFactory;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.timeout.IdleStateHandler;
import io.netty.util.ReferenceCountUtil;
import io.netty.util.ReferenceCounted;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

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

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private final CommonConfiguration commonConfiguration;
    private final TLSConfiguration tlsConfiguration;
    private final EventLoopFactory eventLoopFactory;
    private final L4Balance l4Balance;

    private ConcurrentLinkedQueue<ByteBuf> backlog = new ConcurrentLinkedQueue<>();
    private boolean channelActive = false;
    private Channel downstreamChannel;
    private Backend backend;

    UpstreamHandler(CommonConfiguration commonConfiguration, TLSConfiguration tlsConfiguration, EventLoopFactory eventLoopFactory,
                    L4Balance l4Balance) {
        this.commonConfiguration = commonConfiguration;
        this.tlsConfiguration = tlsConfiguration;
        this.eventLoopFactory = eventLoopFactory;
        this.l4Balance = l4Balance;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        Bootstrap bootstrap = BootstrapFactory.getTCP(commonConfiguration, eventLoopFactory.getChildGroup(), ctx.alloc());
        backend = l4Balance.getBackend((InetSocketAddress) ctx.channel().remoteAddress());
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();
                ch.pipeline().addFirst(new IdleStateHandler(timeout, timeout, timeout));

                if (tlsConfiguration != null) {
                    ch.pipeline().addLast(tlsConfiguration.getDefault().getSslContext()
                            .newHandler(ctx.alloc(), backend.getSocketAddress().getHostName(), backend.getSocketAddress().getPort()));
                }

                ch.pipeline().addLast(new DownstreamHandler(ctx.channel(), backend));
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        downstreamChannel = channelFuture.channel();

        // Listener for writing Backlog
        channelFuture.addListener((ChannelFutureListener) future -> {

            /*
             * If we're connected to the backend, then we'll send all backlog to backend.
             *
             * If we're not connected to the backend, close everything.
             */
            if (future.isSuccess()) {
                eventLoopFactory.getChildGroup().next().execute(() -> {

                    backlog.forEach(packet -> {
                        backend.incBytesWritten(packet.readableBytes());
                        downstreamChannel.writeAndFlush(packet).addListener((ChannelFutureListener) cf -> {
                            if (!cf.isSuccess()) {
                                packet.release();
                            }
                        });
                        backlog.remove(packet);
                    });

                    channelActive = true;
                    backlog = null;
                });
            } else {
                downstreamChannel.close();
                ctx.channel().close();
            }
        });
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ByteBuf byteBuf = (ByteBuf) msg;
        if (channelActive) {
            backend.incBytesWritten(byteBuf.readableBytes());
            downstreamChannel.writeAndFlush(byteBuf);
            return;
        } else if (backlog != null && backlog.size() < commonConfiguration.getTransportConfiguration().getDataBacklog()) {
            backlog.add(byteBuf);
            return;
        }
        byteBuf.release();
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            InetSocketAddress socketAddress = ((InetSocketAddress) ctx.channel().remoteAddress());
            if (backend == null) {
                logger.info("Closing Upstream {}",
                        socketAddress.getAddress().getHostAddress() + ":" + socketAddress.getPort());
            } else {
                logger.info("Closing Upstream {} and Downstream {} Channel",
                        socketAddress.getAddress().getHostAddress() + ":" + socketAddress.getPort(),
                        backend.getSocketAddress().getAddress().getHostAddress() + ":" + backend.getSocketAddress().getPort());
            }
        }

        if (ctx.channel().isActive()) {
            ctx.channel().close();
        }

        if (downstreamChannel.isActive()) {
            downstreamChannel.close();
        }

        if (backlog != null) {
            for (ByteBuf byteBuf : backlog) {
                if (byteBuf.refCnt() > 0) {
                    byteBuf.release();
                }
            }
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
