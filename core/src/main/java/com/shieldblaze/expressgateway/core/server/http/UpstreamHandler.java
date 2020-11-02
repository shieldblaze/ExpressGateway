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

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.CommonConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.core.server.http.adapter.OutboundAdapter;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandlerBuilder;
import com.shieldblaze.expressgateway.core.server.http.compression.HTTPContentDecompressor;
import com.shieldblaze.expressgateway.core.utils.BootstrapFactory;
import com.shieldblaze.expressgateway.core.utils.ChannelUtils;
import com.shieldblaze.expressgateway.core.utils.EventLoopFactory;
import com.shieldblaze.expressgateway.core.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
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
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.logging.LogLevel;
import io.netty.handler.logging.LoggingHandler;
import io.netty.handler.pcap.PcapWriteHandler;
import io.netty.handler.ssl.SslHandler;
import io.netty.handler.timeout.IdleStateHandler;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;

public final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    final Map<Integer, String> acceptEncodingMap = new ConcurrentHashMap<>();
    final boolean isHTTP2;
    private final HTTPBalance HTTPBalance;
    private final CommonConfiguration commonConfiguration;
    private final EventLoopFactory eventLoopFactory;
    private final HTTPConfiguration httpConfiguration;
    private final int maxDataBacklog;
    Channel upstreamChannel;
    Channel downstreamChannel;
    Backend backend;
    InetSocketAddress upstreamAddress;
    InetSocketAddress downstreamAddress;
    private ConcurrentLinkedQueue<Object> backlog = new ConcurrentLinkedQueue<>();
    private long bytesReceived = 0L;
    private boolean channelActive = false;

    private final HTTPLoadBalancer httpLoadBalancer;

    /**
     * Create a new {@link UpstreamHandler} Instance with {@code isHTTP2} set to {@code false}
     *
     * @param httpLoadBalancer {@link HTTPLoadBalancer} Instance
     */
    public UpstreamHandler(HTTPLoadBalancer httpLoadBalancer) {
        this(httpLoadBalancer, false);
    }

    /**
     * Create a new {@link UpstreamHandler} Instance
     *
     * @param httpLoadBalancer {@link HTTPLoadBalancer} Instance
     * @param isHTTP2          Set to {@code true} if connection is established over HTTP/2 else set to {@code false}
     */
    public UpstreamHandler(HTTPLoadBalancer httpLoadBalancer, boolean isHTTP2) {
        this.HTTPBalance = httpLoadBalancer.getL7Balance();
        this.commonConfiguration = httpLoadBalancer.getCommonConfiguration();
        this.eventLoopFactory = httpLoadBalancer.getEventLoopFactory();
        this.httpConfiguration = httpLoadBalancer.getHTTPConfiguration();
        this.isHTTP2 = isHTTP2;
        this.maxDataBacklog = commonConfiguration.getTransportConfiguration().getDataBacklog();
        this.httpLoadBalancer = httpLoadBalancer;
    }

    @Override
    @SuppressWarnings("lgtm[java/dereferenced-value-may-be-null]")
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (upstreamAddress == null) {
            upstreamAddress = (InetSocketAddress) ctx.channel().remoteAddress();
        }

        if (upstreamChannel == null) {
            upstreamChannel = ctx.channel();
        }

        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;
            HttpHeaders headers = request.headers();

            // Get Backend
            if (backend == null) {
                backend = HTTPBalance.getResponse(new HTTPBalanceRequest((InetSocketAddress) ctx.channel().remoteAddress(), headers)).getBackend();
            }

            // If Backend is not found, return `BAD_GATEWAY_502` response.
            if (backend == null) {
                // If request have `Keep-Alive`, return `Keep-Alive` else `Close` response.
                if (HttpUtil.isKeepAlive(request)) {
                    ctx.channel().writeAndFlush(HTTPResponses.BAD_GATEWAY_502.retainedDuplicate());
                } else {
                    ctx.channel().writeAndFlush(HTTPResponses.BAD_GATEWAY_502.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                }
                return;
            }

            // If Downstream Channel is not active yet, we'll create new.
            if (!channelActive) {
                newChannel(ctx.alloc());
            }

            /*
             * If Upstream is HTTP/2 then we'll map the Stream ID with `ACCEPT_ENCODING` so we can
             * later pass it to 'DownstreamHandler` to put `CONTENT_ENCODING` for {@link HTTP2ContentCompressor}
             * to compress it.
             */
            if (isHTTP2) {
                String acceptEncoding = headers.get(HttpHeaderNames.ACCEPT_ENCODING);
                if (acceptEncoding != null) {
                    int streamId = headers.getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
                    acceptEncodingMap.put(streamId, acceptEncoding);
                }
            }

            headers.remove(HttpHeaderNames.UPGRADE);
            headers.set("X-Forwarded-For", upstreamAddress.getAddress().getHostAddress());
            headers.set(HttpHeaderNames.HOST, backend.getCluster().getHostname());
            headers.set(HttpHeaderNames.ACCEPT_ENCODING, "gzip, deflate");

            if (channelActive) {
                if (request.headers().contains(HttpHeaderNames.CONTENT_LENGTH)) {
                    backend.incBytesWritten(Long.parseLong(request.headers().get(HttpHeaderNames.CONTENT_LENGTH)));
                }
                downstreamChannel.writeAndFlush(msg);
            } else if (backlog != null && backlog.size() < maxDataBacklog) {
                backlog.add(msg);
            }

            bytesReceived = 0L;
        } else if (msg instanceof HttpContent) {
            HttpContent content = (HttpContent) msg;

            bytesReceived += content.content().readableBytes();
            if (bytesReceived > httpConfiguration.getMaxContentLength()) {
                ctx.writeAndFlush(HTTPResponses.TOO_LARGE_413.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            if (channelActive) {
                backend.incBytesWritten(content.content().readableBytes());
                downstreamChannel.writeAndFlush(content);
                return;
            } else if (backlog != null && backlog.size() < maxDataBacklog) {
                backlog.add(content);
                return;
            }

            content.release();
        }
    }

    private void newChannel(ByteBufAllocator allocator) {
        UpstreamHandler upstreamHandler = this;

        Bootstrap bootstrap = BootstrapFactory.getTCP(commonConfiguration, eventLoopFactory.getChildGroup(), allocator);
        bootstrap.handler(new ChannelInitializer<SocketChannel>() {
            @Override
            protected void initChannel(SocketChannel ch) {
                ChannelPipeline pipeline = ch.pipeline();

                int timeout = commonConfiguration.getTransportConfiguration().getConnectionIdleTimeout();
                pipeline.addFirst("IdleStateHandler", new IdleStateHandler(timeout, timeout, timeout));

                if (httpLoadBalancer.getTlsClient() == null) {
                    pipeline.addLast("HTTPClientCodec", HTTPUtils.newClientCodec(httpConfiguration));
                    pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());
                    pipeline.addLast("DownstreamHandler", new DownstreamHandler(upstreamHandler));
                } else {
                    String hostname = backend.getSocketAddress().getHostName();
                    int port = backend.getSocketAddress().getPort();
                    SslHandler sslHandler = httpLoadBalancer.getTlsClient().getDefault().getSslContext().newHandler(allocator, hostname, port);

                    DownstreamHandler downstreamHandler = new DownstreamHandler(upstreamHandler);

                    ALPNHandler alpnHandler = ALPNHandlerBuilder.newBuilder()
                            // HTTP/2 Handlers
                            .withHTTP2ChannelHandler("HTTP2Handler", HTTPUtils.clientH2Handler(httpConfiguration))
                            .withHTTP2ChannelHandler("OutboundAdapter", new OutboundAdapter())
                            .withHTTP2ChannelHandler("DownstreamHandler", downstreamHandler)
                            // HTTP/1.1 Handlers
                            .withHTTP1ChannelHandler("HTTPClientCodec", HTTPUtils.newClientCodec(httpConfiguration))
                            .withHTTP1ChannelHandler("HTTPContentDecompressor", new HTTPContentDecompressor())
                            .withHTTP1ChannelHandler("DownstreamHandler", downstreamHandler)
                            .build();

                    pipeline.addLast("TLSHandler", sslHandler);
/*                    try {
                        pipeline.addLast(new PcapWriteHandler(new FileOutputStream("D://pcap.pcap")));
                    } catch (FileNotFoundException e) {
                        e.printStackTrace();
                    }*/
                    pipeline.addLast("ALPNHandler", alpnHandler);
                }
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        downstreamChannel = channelFuture.channel();

        channelFuture.addListener((ChannelFutureListener) future -> {
            if (future.isSuccess()) {
                downstreamAddress = (InetSocketAddress) downstreamChannel.remoteAddress();

                downstreamChannel.pipeline().get(ALPNHandler.class).getALPNProtocol().whenCompleteAsync((protocol, throwable) -> {
                    if (throwable == null) {
                        backlog.forEach(object -> downstreamChannel.writeAndFlush(object));
                        channelActive = true;
                        backlog = null;
                    } else {
                        ChannelUtils.closeChannels(upstreamChannel, downstreamChannel);
                    }
                }, future.channel().eventLoop());

            } else {
                ChannelUtils.closeChannels(upstreamChannel, downstreamChannel);
            }
        });
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (logger.isInfoEnabled()) {
            if (upstreamAddress != null) {
                if (downstreamAddress == null) {
                    logger.info("Closing Upstream {}", upstreamAddress.getAddress().getHostAddress() + ":" + upstreamAddress.getPort());
                } else {
                    logger.info("Closing Upstream {} and Downstream {} Channel",
                            upstreamAddress.getAddress().getHostAddress() + ":" + upstreamAddress.getPort(),
                            downstreamAddress.getAddress().getHostAddress() + ":" + downstreamAddress.getPort());
                }
            }
        }

        ChannelUtils.closeChannels(ctx.channel(), downstreamChannel);

        if (backlog != null) {
            for (Object httpObject : backlog) {
                ReferenceCountedUtil.silentFullRelease(httpObject);
            }
            backlog = null;
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
