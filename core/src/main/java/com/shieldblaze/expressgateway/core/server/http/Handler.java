package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.core.configuration.CommonConfiguration;
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
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpObject;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.logging.LogLevel;
import io.netty.handler.logging.LoggingHandler;
import io.netty.handler.stream.ChunkedWriteHandler;
import io.netty.handler.timeout.IdleStateHandler;

public final class Handler extends SimpleChannelInboundHandler<HttpObject> {

    private int maxContentLength = 8196;

    private long bytesReceived = 0L;

    private Channel downstreamChannel;
    private ChannelFuture channelFuture;

    private final L7Balance l7Balance;
    private final CommonConfiguration commonConfiguration;
    private final TLSConfiguration tlsConfiguration;
    private final EventLoopFactory eventLoopFactory;

    public Handler(L7Balance l7Balance, CommonConfiguration commonConfiguration, TLSConfiguration tlsConfiguration,
                   EventLoopFactory eventLoopFactory) {
        this.l7Balance = l7Balance;
        this.commonConfiguration = commonConfiguration;
        this.tlsConfiguration = tlsConfiguration;
        this.eventLoopFactory = eventLoopFactory;
    }

    @Override
    protected void channelRead0(ChannelHandlerContext ctx, HttpObject msg) throws InterruptedException {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;
            boolean keepAlive = HttpUtil.isKeepAlive(request);

            // Find Backend
            Backend backend = l7Balance.getBackend(request);

            // If Backend is not found, return `BAD_GATEWAY` response.
            if (backend == null) {
                // If request have `Keep-Alive`, return `Keep-Alive` else `Close` response.
                if (keepAlive) {
                    ctx.writeAndFlush(HttpResponses.BAD_GATEWAY_KEEP_ALIVE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else {
                    ctx.writeAndFlush(HttpResponses.BAD_GATEWAY.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                }
                return;
            }

            // If Connection with Downstream is not established yet, we'll create new.
            if (downstreamChannel == null) {
                newChannel(backend, ctx.alloc(), ctx.channel());
            }

            request.headers().add("X-Forwarded-For", ctx.channel().remoteAddress());

            downstreamChannel.writeAndFlush(msg).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
        } else if (msg instanceof HttpContent) {
            HttpContent content = (HttpContent) msg;

            bytesReceived += content.content().readableBytes();
            if (bytesReceived > maxContentLength) {
                ctx.writeAndFlush(HttpResponses.TOO_LARGE.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            downstreamChannel.writeAndFlush(content).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
        }
    }

    private void newChannel(Backend backend, ByteBufAllocator byteBufAllocator, Channel channel) throws InterruptedException {
        Bootstrap bootstrap = BootstrapFactory.getTCP(commonConfiguration, eventLoopFactory.getChildGroup(), byteBufAllocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                pipeline.addFirst(new LoggingHandler(LogLevel.DEBUG));

                int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();
                pipeline.addFirst(new IdleStateHandler(timeout, timeout, timeout));

                if (tlsConfiguration != null) {
                    pipeline.addLast(tlsConfiguration.getDefault().getSslContext().newHandler(byteBufAllocator,
                            backend.getSocketAddress().getHostName(), backend.getSocketAddress().getPort()));
                }

                pipeline.addLast(new HttpClientCodec());
//                pipeline.addLast(new HttpContentCompressor());
//                pipeline.addLast(new HttpContentDecompressor());
//                pipeline.addLast(new ChunkedWriteHandler());
                pipeline.addLast(new DownstreamHandler(channel));
            }
        });

        channelFuture = bootstrap.connect(backend.getSocketAddress());
        downstreamChannel = channelFuture.channel();
        channelFuture.sync();
    }
}
