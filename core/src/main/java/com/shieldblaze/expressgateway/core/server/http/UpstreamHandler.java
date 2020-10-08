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
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
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
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpObject;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpUtil;
import io.netty.handler.codec.http2.Http2Frame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.codec.http2.HttpToHttp2ConnectionHandler;
import io.netty.handler.codec.http2.InboundHttp2ToHttpObjectAdapter;
import io.netty.handler.ssl.SslHandler;
import io.netty.handler.timeout.IdleStateHandler;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * <p>
 * {@link UpstreamHandler} handles incoming HTTP requests.
 * It can handle HTTP/2 and HTTP/1.1 together. For {@link UpstreamHandler} to handle
 * HTTP/2 requests, {@link #isHTTP2} must be set to {@code true}. For HTTP/2 traffic,
 * Client must use HTTP/2 over TLS (h2). And for {@link UpstreamHandler}
 * to handle HTTP/1.1 requests, {@link #isHTTP2} must be set to {@code false}.
 * </p>
 * <p></p>
 * <p>
 * <p> How HTTP/2 traffic is handled: </p>
 * <p>
 * When HTTP/2 request arrives, {@link InboundHttp2ToHttpObjectAdapter} converts it into
 * {@link HttpObject}. When {@link HttpObject} arrives at {@link UpstreamHandler}, we take Stream ID
 * from {@link HttpRequest} inside {@code x-http2-stream-id} header and map it with {@code ACCEPT-ENCODING} header
 * in {@link #acceptEncodingMap}. Then we connect to {@link Backend} server. If {@link TLSConfiguration}
 * is supplied, then we use {@link ALPNClientHandler} while connecting to the {@link Backend}.
 *         <ul>
 *             <li>
 *                  If Backend is connected to HTTP/2, we will write {@link HttpObject} and {@link HttpToHttp2ConnectionHandler}
 *                  will convert it to {@link Http2Frame}. {@link HttpToHttp2ConnectionHandler} uses same the StreamID which is
 *                  present in {@link HttpRequest} inside {@code x-http2-stream-id} header. When response comes back from {@link Backend},
 *                  it is handled by {@link DownstreamHandler}. {@link DownstreamHandler} checks if {@link UpstreamHandler} uses HTTP/2,
 *                  If yes then we'll fetch {@code ACCEPT-ENCODING} for the actual request from {@link #acceptEncodingMap} using Stream ID
 *                  and set {@code CONTENT-ENCODING} if response is compressible and {@code ACCEPT-ENCODING} accepts the compression method.
 *                  Then we'll forward the request and {@link HttpToHttp2ConnectionHandler} will receive the response and compress if
 *                  {@code CONTENT-ENCODING} header is set and finally pass the request back to origin.
 *             </li>
 *             <li>
 *                 If Backend is connected to HTTP/1.1 when we'll use {@link HTTPTranslationAdapter} for Translation
 *                 between {@link UpstreamHandler} HTTP/2 to {@link DownstreamHandler} HTTP/1.1.
 *             </li>
 *         </ul>
 *     </p>
 * </p>
 */
final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private final Map<Integer, String> acceptEncodingMap = new ConcurrentHashMap<>();
    private ConcurrentLinkedQueue<Object> backlog = new ConcurrentLinkedQueue<>();

    private long bytesReceived = 0L;
    private boolean channelActive = false;
    private Channel downstreamChannel;
    private Backend backend;

    private final L7Balance l7Balance;
    private final CommonConfiguration commonConfiguration;
    private final TLSConfiguration tlsClient;
    private final EventLoopFactory eventLoopFactory;
    private final HTTPConfiguration httpConfiguration;
    private final boolean isHTTP2;

    UpstreamHandler(HTTPLoadBalancer httpLoadBalancer, TLSConfiguration tlsClient) {
        this(httpLoadBalancer, tlsClient, false);
    }

    UpstreamHandler(HTTPLoadBalancer httpLoadBalancer, TLSConfiguration tlsClient, boolean isHTTP2) {
        this.l7Balance = httpLoadBalancer.getL7Balance();
        this.commonConfiguration = httpLoadBalancer.getCommonConfiguration();
        this.eventLoopFactory = httpLoadBalancer.getEventLoopFactory();
        this.httpConfiguration = httpLoadBalancer.getHTTPConfiguration();
        this.tlsClient = tlsClient;
        this.isHTTP2 = isHTTP2;
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

            // If Downstream Channel is not active yet, we'll create new.
            if (!channelActive) {
                newChannel(backend, ctx.alloc(), ctx.channel());
            }

            /*
             * If Upstream is HTTP/2 then we'll map the Stream ID with `ACCEPT_ENCODING` so we can
             * later pass it to 'DownstreamHandler` to put `CONTENT_ENCODING` for {@link HTTP2ContentCompressor}
             * to compress it.
             */
            if (isHTTP2) {
                String acceptEncoding = request.headers().get(HttpHeaderNames.ACCEPT_ENCODING);
                if (acceptEncoding != null) {
                    int streamId = request.headers().getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
                    acceptEncodingMap.put(streamId, acceptEncoding);
                }
            }

            request.headers().set("X-Forwarded-For", ((InetSocketAddress) ctx.channel().remoteAddress()).getAddress().getHostAddress());
            request.headers().set(HttpHeaderNames.HOST, backend.getHostname());
            request.headers().set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");

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
                pipeline.addFirst("IdleStateHandler", new IdleStateHandler(timeout, timeout, timeout));

                if (tlsClient == null) {
                    pipeline.addLast(HTTPCodecs.newClient(httpConfiguration));
                    pipeline.addLast("HTTPContentCompressor", new HTTPContentCompressor(4, 6, 15, 8, 0));
                    pipeline.addLast("HTTPContentDecompressor", new HTTPContentDecompressor());
                    pipeline.addLast("DownstreamHandler", new DownstreamHandler(channel, backend, acceptEncodingMap, isHTTP2));
                } else {
                    String hostname = backend.getSocketAddress().getHostName();
                    int port = backend.getSocketAddress().getPort();
                    SslHandler sslHandler = tlsClient.getDefault().getSslContext().newHandler(byteBufAllocator, hostname, port);

                    pipeline.addLast("TLSHandler", sslHandler);
                    pipeline.addLast("ALPNServerHandler", new ALPNClientHandler(httpConfiguration,
                            new DownstreamHandler(channel, backend, acceptEncodingMap, isHTTP2), ch.newPromise(), isHTTP2));
                }
            }
        });

        ChannelFuture channelFuture = bootstrap.connect(backend.getSocketAddress());
        downstreamChannel = channelFuture.channel();

        channelFuture.addListener((ChannelFutureListener) _channelFuture -> {
            if (_channelFuture.isSuccess()) {

                downstreamChannel.pipeline().get(ALPNClientHandler.class).promise().addListener((ChannelFutureListener) alpnFuture -> {
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
