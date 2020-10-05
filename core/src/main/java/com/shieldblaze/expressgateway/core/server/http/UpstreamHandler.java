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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.core.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.core.netty.BootstrapFactory;
import com.shieldblaze.expressgateway.core.netty.EventLoopFactory;
import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.l7.L7Balance;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.timeout.IdleStateHandler;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentLinkedQueue;

final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private ConcurrentLinkedQueue<Object> backlog = new ConcurrentLinkedQueue<>();

    private long bytesReceived = 0L;
    private boolean channelActive = false;
    private Channel downstreamChannel;
    private Backend backend;

    private final L7Balance l7Balance;
    private final CommonConfiguration commonConfiguration;
    private final TLSConfiguration tlsConfiguration;
    private final EventLoopFactory eventLoopFactory;
    private final HTTPConfiguration httpConfiguration;

    UpstreamHandler(L7Balance l7Balance, CommonConfiguration commonConfiguration, TLSConfiguration tlsConfiguration,
                    EventLoopFactory eventLoopFactory, HTTPConfiguration httpConfiguration) {
        this.l7Balance = l7Balance;
        this.commonConfiguration = commonConfiguration;
        this.tlsConfiguration = tlsConfiguration;
        this.eventLoopFactory = eventLoopFactory;
        this.httpConfiguration = httpConfiguration;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            // Get Backend
            backend = l7Balance.getBackend(request);

            // If Backend is not found, return `BAD_GATEWAY` response.
            if (backend == null) {
                // If request have `Keep-Alive`, return `Keep-Alive` else `Close` response.
                if (HttpUtil.isKeepAlive(request)) {
                    ctx.writeAndFlush(HttpResponses.BAD_GATEWAY_KEEP_ALIVE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else {
                    ctx.writeAndFlush(HttpResponses.BAD_GATEWAY.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                }
                return;
            }

            // If Connection with Downstream is not established yet, we'll create new.
            if (!channelActive) {
                newChannel(backend, ctx.alloc(), ctx.channel());
            }

            request.headers().set("X-Forwarded-For", ((InetSocketAddress) ctx.channel().remoteAddress()).getAddress().getHostAddress());
            request.headers().set(HttpHeaderNames.HOST, backend.getHostname());

            if (channelActive) {
                backend.incBytesWritten(HttpUtil.getContentLength(request, 0));
                downstreamChannel.writeAndFlush(msg).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            } else if (backlog != null && backlog.size() < commonConfiguration.getTransportConfiguration().getDataBacklog()) {
                backlog.add(msg);
            }

            bytesReceived = 0L;
        } else if (msg instanceof HttpContent) {
            HttpContent content = (HttpContent) msg;

            bytesReceived += content.content().readableBytes();
            if (bytesReceived > httpConfiguration.getMaxContentLength()) {
                ctx.writeAndFlush(HttpResponses.TOO_LARGE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            if (channelActive) {
                backend.incBytesWritten(content.content().readableBytes());
                downstreamChannel.writeAndFlush(content).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                return;
            } else if (backlog != null && backlog.size() < commonConfiguration.getTransportConfiguration().getDataBacklog()) {
                backlog.add(content);
                return;
            }

            content.release();
        }
    }

    private void newChannel(Backend backend, ByteBufAllocator byteBufAllocator, Channel channel) {
        Bootstrap bootstrap = BootstrapFactory.getTCP(commonConfiguration, eventLoopFactory.getChildGroup(), byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();
                pipeline.addFirst(new IdleStateHandler(timeout, timeout, timeout));

                if (tlsConfiguration != null) {
                    pipeline.addLast(tlsConfiguration.getDefault().getSslContext().newHandler(byteBufAllocator,
                            backend.getSocketAddress().getHostName(), backend.getSocketAddress().getPort()));

                    pipeline.addLast(new ALPNHandlerClient(httpConfiguration, new DownstreamHandler(channel, backend), ch.newPromise()));
                } else {
                    pipeline.addLast(new HttpClientCodec(), new DownstreamHandler(channel, backend));
                }
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        downstreamChannel = channelFuture.channel();

        channelFuture.addListener((ChannelFutureListener) _channelFuture -> {
            if (_channelFuture.isSuccess()) {

                downstreamChannel.pipeline().get(ALPNHandlerClient.class).promise().addListener((ChannelFutureListener) alpnFuture -> {
                    if (alpnFuture.isSuccess()) {
                        backlog.forEach(httpObject -> downstreamChannel.writeAndFlush(httpObject)
                                .addListener((ChannelFutureListener) writeFuture -> {
                                    if (!writeFuture.isSuccess()) {
                                        if (httpObject instanceof HttpContent && ((HttpContent) httpObject).refCnt() > 0) {
                                            ReferenceCountUtil.release(httpObject);
                                        }
                                        if (channel.isActive()) {
                                            channel.close();
                                        }
                                    }
                                    backlog.remove(httpObject);
                                }));

                        channelActive = true;
                        backlog = null;
                    } else {
                        downstreamChannel.close();
                        channel.close();
                    }
                });

            } else {
                downstreamChannel.close();
                channel.close();
            }
        });
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

        if (downstreamChannel != null && downstreamChannel.isActive()) {
            downstreamChannel.close();
        }

        if (backlog != null) {
            for (Object httpObject : backlog) {
                if (httpObject instanceof HttpContent && ((HttpContent) httpObject).refCnt() > 0) {
                    ReferenceCountUtil.release(httpObject);
                }
            }
            backlog = null;
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
