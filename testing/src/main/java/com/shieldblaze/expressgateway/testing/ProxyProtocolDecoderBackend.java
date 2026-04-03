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

import com.shieldblaze.expressgateway.configuration.transport.ProxyProtocolMode;
import com.shieldblaze.expressgateway.core.handlers.ProxyProtocolHandler;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.util.Objects;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Reusable embedded TCP backend server for proxy protocol integration tests.
 *
 * <p>Starts a real Netty server on an ephemeral port, installs the production
 * {@link ProxyProtocolHandler} in the pipeline, and captures the decoded
 * {@link ProxyProtocolHandler#REAL_CLIENT_ADDRESS} via a
 * {@link CompletableFuture} after the first connection completes PP decoding.
 * After PP decoding the server echoes all received data back to the sender.</p>
 *
 * <p>Also tracks the total number of PP headers received via
 * {@link #headersReceived()} so integration tests can verify that pooled
 * (keep-alive) connections do not re-send headers on each request.</p>
 *
 * <p>Usage:</p>
 * <pre>
 *   try (ProxyProtocolDecoderBackend backend =
 *           new ProxyProtocolDecoderBackend(ProxyProtocolMode.AUTO)) {
 *       backend.start();
 *       // ... connect a client to backend.port(), send data ...
 *       InetSocketAddress addr = backend.lastParsedAddress().get(5, TimeUnit.SECONDS);
 *   }
 * </pre>
 */
public final class ProxyProtocolDecoderBackend implements Closeable {

    private final ProxyProtocolMode mode;

    private NioEventLoopGroup bossGroup;
    private NioEventLoopGroup workerGroup;
    private Channel serverChannel;
    private volatile int boundPort = -1;

    /**
     * Completed with the real client address extracted from the first PP header
     * received on any connection. Reset by calling {@link #resetParsedAddress()}.
     */
    private volatile CompletableFuture<InetSocketAddress> parsedAddressFuture =
            new CompletableFuture<>();

    /**
     * Total number of successful PP headers decoded since the server started.
     * Useful for verifying pooled connections do not re-send headers.
     */
    private final AtomicInteger headersReceived = new AtomicInteger();

    /**
     * @param mode the proxy protocol decode mode (must not be {@link ProxyProtocolMode#OFF})
     * @throws NullPointerException     if {@code mode} is null
     * @throws IllegalArgumentException if {@code mode} is {@link ProxyProtocolMode#OFF}
     */
    public ProxyProtocolDecoderBackend(ProxyProtocolMode mode) {
        this.mode = Objects.requireNonNull(mode, "ProxyProtocolMode");
        if (mode == ProxyProtocolMode.OFF) {
            throw new IllegalArgumentException("ProxyProtocolDecoderBackend requires a non-OFF mode");
        }
    }

    /**
     * Bind the server to an ephemeral port and start accepting connections.
     *
     * @return this instance for chaining
     * @throws InterruptedException if the bind is interrupted
     */
    public ProxyProtocolDecoderBackend start() throws InterruptedException {
        bossGroup = new NioEventLoopGroup(1);
        workerGroup = new NioEventLoopGroup(2);

        ServerBootstrap bootstrap = new ServerBootstrap()
                .group(bossGroup, workerGroup)
                .channel(NioServerSocketChannel.class)
                .childHandler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        ch.pipeline().addLast("pp-decoder", new ProxyProtocolHandler(mode));
                        ch.pipeline().addLast("address-capturer", new AddressCapturerHandler());
                        ch.pipeline().addLast("echo", new EchoHandler());
                    }
                });

        serverChannel = bootstrap.bind(0).sync().channel();
        boundPort = ((InetSocketAddress) serverChannel.localAddress()).getPort();
        return this;
    }

    /**
     * Return the port the server is bound to.
     *
     * @throws IllegalStateException if {@link #start()} has not been called yet
     */
    public int port() {
        if (boundPort == -1) {
            throw new IllegalStateException("ProxyProtocolDecoderBackend not started");
        }
        return boundPort;
    }

    /**
     * Return a {@link CompletableFuture} that is completed with the real client address
     * extracted from the first PP header decoded on any connection.
     *
     * <p>If no PP header was received (e.g. PROXY UNKNOWN or v2 LOCAL), the future
     * is completed with {@code null}.</p>
     *
     * <p>Call {@link #resetParsedAddress()} to obtain a fresh future before the next
     * connection if running multiple assertions in sequence.</p>
     */
    public CompletableFuture<InetSocketAddress> lastParsedAddress() {
        return parsedAddressFuture;
    }

    /**
     * Reset the parsed-address future so the next connection's PP header can be captured.
     * Should be called between test scenarios when reusing the same server instance.
     */
    public void resetParsedAddress() {
        parsedAddressFuture = new CompletableFuture<>();
    }

    /**
     * Return the total number of PP headers that have been decoded since the server started.
     * Incremented once per connection after the {@link ProxyProtocolHandler} has run.
     */
    public int headersReceived() {
        return headersReceived.get();
    }

    /**
     * Reset the header-received counter to zero.
     */
    public void resetHeadersReceived() {
        headersReceived.set(0);
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

    // -------------------------------------------------------------------------
    // Pipeline handlers
    // -------------------------------------------------------------------------

    /**
     * Captures the real client address after {@link ProxyProtocolHandler} has decoded
     * the PP header. The PP header is decoded in {@code channelRead()}, not
     * {@code channelActive()}, so this handler must also use {@code channelRead()}
     * to read the attribute after the decoder has set it and removed itself.
     *
     * <p>The first {@code channelRead} after the PP handler fires is the first
     * application-level data. At that point the attribute is guaranteed to be set
     * (or null for UNKNOWN/LOCAL). This handler captures the attribute, increments
     * the header counter, removes itself, and forwards the data.</p>
     */
    private final class AddressCapturerHandler extends ChannelInboundHandlerAdapter {

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
            headersReceived.incrementAndGet();
            InetSocketAddress realAddr = ctx.channel().attr(ProxyProtocolHandler.REAL_CLIENT_ADDRESS).get();
            parsedAddressFuture.complete(realAddr);
            ctx.pipeline().remove(this);
            ctx.fireChannelRead(msg);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            parsedAddressFuture.completeExceptionally(cause);
            ctx.close();
        }
    }

    /**
     * Simple echo handler — reflects every received {@link ByteBuf} back to the sender.
     * Ownership of {@code msg} transfers from {@code channelRead} to {@code writeAndFlush},
     * so no explicit {@code retain()} is needed (and would cause a ByteBuf leak).
     */
    private static final class EchoHandler extends ChannelInboundHandlerAdapter {

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) {
            ctx.writeAndFlush(msg);
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            ctx.close();
        }
    }
}
