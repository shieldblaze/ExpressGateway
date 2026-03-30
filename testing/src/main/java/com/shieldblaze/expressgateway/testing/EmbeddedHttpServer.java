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
package com.shieldblaze.expressgateway.testing;

import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpObjectAggregator;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpServerCodec;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.function.Function;

/**
 * Lightweight embedded HTTP/1.1 server for integration tests.
 * Starts a Netty-based server on an ephemeral port and responds with
 * a configurable response handler.
 *
 * <p>Supports both plaintext and TLS modes. Uses NIO transport for
 * maximum portability across test environments.</p>
 *
 * <p>Example:</p>
 * <pre>
 *   try (EmbeddedHttpServer server = EmbeddedHttpServer.builder()
 *           .responseBody("OK")
 *           .build()
 *           .start()) {
 *       // server.port() returns the actual bound port
 *       HttpResponse resp = httpClient.send(request, BodyHandlers.ofString());
 *   }
 * </pre>
 */
public final class EmbeddedHttpServer implements Closeable {

    private final int port;
    private final SslContext sslContext;
    private final Function<FullHttpRequest, FullHttpResponse> handler;
    private NioEventLoopGroup bossGroup;
    private NioEventLoopGroup workerGroup;
    private Channel serverChannel;
    private volatile int boundPort = -1;

    private EmbeddedHttpServer(int port, SslContext sslContext,
                               Function<FullHttpRequest, FullHttpResponse> handler) {
        this.port = port;
        this.sslContext = sslContext;
        this.handler = handler;
    }

    /**
     * Start the server and bind to the configured port.
     *
     * @return this server instance for chaining
     */
    public EmbeddedHttpServer start() throws InterruptedException {
        bossGroup = new NioEventLoopGroup(1);
        workerGroup = new NioEventLoopGroup(2);

        ServerBootstrap bootstrap = new ServerBootstrap()
                .group(bossGroup, workerGroup)
                .channel(NioServerSocketChannel.class)
                .childHandler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        if (sslContext != null) {
                            ch.pipeline().addLast("ssl", sslContext.newHandler(ch.alloc()));
                        }
                        ch.pipeline().addLast("codec", new HttpServerCodec());
                        ch.pipeline().addLast("aggregator", new HttpObjectAggregator(1048576));
                        ch.pipeline().addLast("handler", new RequestHandler(handler));
                    }
                });

        serverChannel = bootstrap.bind(port).sync().channel();
        boundPort = ((InetSocketAddress) serverChannel.localAddress()).getPort();
        return this;
    }

    /**
     * Return the actual port the server is bound to. Useful when binding to port 0.
     */
    public int port() {
        if (boundPort == -1) {
            throw new IllegalStateException("Server not started yet");
        }
        return boundPort;
    }

    /**
     * Return whether TLS is enabled on this server.
     */
    public boolean isTls() {
        return sslContext != null;
    }

    /**
     * Return the base URL (http or https) for this server.
     */
    public String baseUrl() {
        String scheme = isTls() ? "https" : "http";
        return scheme + "://127.0.0.1:" + port();
    }

    @Override
    public void close() {
        if (serverChannel != null) {
            serverChannel.close().syncUninterruptibly();
        }
        if (bossGroup != null) {
            bossGroup.shutdownGracefully().syncUninterruptibly();
        }
        if (workerGroup != null) {
            workerGroup.shutdownGracefully().syncUninterruptibly();
        }
    }

    /**
     * Create a builder for configuring the embedded server.
     */
    public static Builder builder() {
        return new Builder();
    }

    public static final class Builder {
        private int port;
        private SslContext sslContext;
        private Function<FullHttpRequest, FullHttpResponse> handler;
        private String staticBody = "OK";
        private HttpResponseStatus staticStatus = HttpResponseStatus.OK;

        private Builder() {
        }

        /**
         * Set the port to bind. Default is 0 (ephemeral).
         */
        public Builder port(int port) {
            this.port = port;
            return this;
        }

        /**
         * Enable TLS with the given self-signed certificate.
         */
        public Builder tls(SelfSignedCertificate cert) {
            try {
                this.sslContext = SslContextBuilder.forServer(cert.privateKey(), cert.certificate()).build();
            } catch (Exception ex) {
                throw new RuntimeException("Failed to build SslContext", ex);
            }
            return this;
        }

        /**
         * Enable TLS with the given SslContext.
         */
        public Builder sslContext(SslContext sslContext) {
            this.sslContext = sslContext;
            return this;
        }

        /**
         * Set a static response body. Default is "OK".
         */
        public Builder responseBody(String body) {
            this.staticBody = body;
            return this;
        }

        /**
         * Set a static response status. Default is 200.
         */
        public Builder responseStatus(HttpResponseStatus status) {
            this.staticStatus = status;
            return this;
        }

        /**
         * Set a custom request handler that maps each request to a response.
         * Overrides responseBody/responseStatus.
         */
        public Builder handler(Function<FullHttpRequest, FullHttpResponse> handler) {
            this.handler = handler;
            return this;
        }

        public EmbeddedHttpServer build() {
            Function<FullHttpRequest, FullHttpResponse> effectiveHandler = this.handler;
            if (effectiveHandler == null) {
                String body = Objects.requireNonNull(staticBody);
                HttpResponseStatus status = Objects.requireNonNull(staticStatus);
                effectiveHandler = req -> {
                    byte[] bytes = body.getBytes(StandardCharsets.UTF_8);
                    FullHttpResponse resp = new DefaultFullHttpResponse(
                            HttpVersion.HTTP_1_1, status, Unpooled.wrappedBuffer(bytes));
                    resp.headers().set("Content-Type", "text/plain; charset=UTF-8");
                    resp.headers().setInt("Content-Length", bytes.length);
                    return resp;
                };
            }
            return new EmbeddedHttpServer(port, sslContext, effectiveHandler);
        }
    }

    private static final class RequestHandler extends SimpleChannelInboundHandler<FullHttpRequest> {

        private final Function<FullHttpRequest, FullHttpResponse> handler;

        RequestHandler(Function<FullHttpRequest, FullHttpResponse> handler) {
            this.handler = handler;
        }

        @Override
        protected void channelRead0(ChannelHandlerContext ctx, FullHttpRequest msg) {
            FullHttpResponse response = handler.apply(msg);
            ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            ctx.close();
        }
    }
}
