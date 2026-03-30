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
package com.shieldblaze.expressgateway.protocol.quic;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.L4FrontListener;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerShutdownTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStopTask;
import com.shieldblaze.expressgateway.core.factory.EventLoopFactory;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.WriteBufferWaterMark;
import io.netty.channel.epoll.EpollDatagramChannel;
import io.netty.channel.epoll.EpollDatagramChannelConfig;
import io.netty.channel.epoll.EpollMode;
import io.netty.channel.group.ChannelGroup;
import io.netty.channel.group.DefaultChannelGroup;
import io.netty.channel.socket.DatagramChannel;
import io.netty.channel.socket.nio.NioDatagramChannel;
import io.netty.channel.unix.UnixChannelOption;
import io.netty.handler.codec.quic.QuicChannel;
import io.netty.handler.codec.quic.QuicServerCodecBuilder;
import io.netty.handler.codec.quic.QuicSslContext;
import io.netty.handler.codec.quic.QuicTokenHandler;
import io.netty.incubator.channel.uring.IOUringDatagramChannel;
import io.netty.util.concurrent.GlobalEventExecutor;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * QUIC Listener for handling incoming QUIC connections over UDP.
 *
 * <p>Uses {@link Bootstrap} (not ServerBootstrap) since QUIC runs over UDP datagrams.
 * The {@link QuicServerCodecBuilder} handles QUIC connection management, connection ID
 * routing, and stream multiplexing within each UDP socket.</p>
 *
 * <p>QUIC transport parameters (RFC 9000 Section 18) are configured from
 * {@link QuicConfiguration}. TLS 1.3 is mandatory for QUIC (RFC 9001) and uses
 * BoringSSL native via {@link QuicSslContext}.</p>
 *
 * <p>On native transports (Epoll, io_uring), multiple UDP sockets are bound with
 * SO_REUSEPORT to distribute incoming packets across EventLoop threads.</p>
 *
 * <p>Application protocols plug in via the {@link QuicChannelInitializerFactory},
 * which provides the {@link ChannelHandler} to install on each new {@link QuicChannel}.
 * For example, HTTP/3 passes a factory that installs {@code Http3ServerConnectionHandler}.</p>
 */
public class QuicListener extends L4FrontListener {

    private static final Logger logger = LogManager.getLogger(QuicListener.class);

    private static final int DEFAULT_DRAIN_TIMEOUT_SECONDS = 30;

    private final List<ChannelFuture> channelFutures = new CopyOnWriteArrayList<>();
    private final ChannelGroup activeQuicConnections = new DefaultChannelGroup(GlobalEventExecutor.INSTANCE);
    private int drainTimeoutSeconds = DEFAULT_DRAIN_TIMEOUT_SECONDS;

    private final QuicSslContext quicSslContext;
    private final QuicChannelInitializerFactory channelInitializerFactory;

    /**
     * Optional custom token handler. When non-null, overrides the default
     * {@link SecureQuicTokenHandler#INSTANCE}. Allows plugging in
     * {@link com.shieldblaze.expressgateway.protocol.quic.retry.StatelessRetryHandler}
     * for stateless retry with AES-GCM tokens and token expiration.
     */
    private volatile QuicTokenHandler customTokenHandler;

    /**
     * Create a new QuicListener.
     *
     * @param quicSslContext              the pre-built QUIC SSL context (BoringSSL native, TLS 1.3)
     * @param channelInitializerFactory   factory for the application protocol handler
     *                                    installed on each new QuicChannel
     */
    public QuicListener(QuicSslContext quicSslContext, QuicChannelInitializerFactory channelInitializerFactory) {
        this.quicSslContext = Objects.requireNonNull(quicSslContext, "QuicSslContext must not be null");
        this.channelInitializerFactory = Objects.requireNonNull(channelInitializerFactory,
                "QuicChannelInitializerFactory must not be null");
    }

    /**
     * Set a custom QUIC token handler (e.g., {@code StatelessRetryHandler}).
     * Must be called before {@link #start()}.
     *
     * @param tokenHandler the custom token handler, or null to use the default
     */
    public void setTokenHandler(QuicTokenHandler tokenHandler) {
        this.customTokenHandler = tokenHandler;
    }

    /**
     * Set the drain timeout in seconds for graceful shutdown.
     */
    public void setDrainTimeoutSeconds(int seconds) {
        this.drainTimeoutSeconds = Math.max(0, seconds);
    }

    /**
     * Get the active QUIC connections channel group for tracking.
     */
    public ChannelGroup activeQuicConnections() {
        return activeQuicConnections;
    }

    @Override
    public L4FrontListenerStartupTask start() {
        L4FrontListenerStartupTask l4FrontListenerStartupEvent = new L4FrontListenerStartupTask();

        if (!channelFutures.isEmpty()) {
            l4FrontListenerStartupEvent.markFailure(
                    new IllegalArgumentException("Listener has already started and cannot be restarted."));
            return l4FrontListenerStartupEvent;
        }

        ConfigurationContext configurationContext = l4LoadBalancer().configurationContext();
        QuicConfiguration quicConfig = configurationContext.quicConfiguration();
        EventLoopFactory eventLoopFactory = l4LoadBalancer().eventLoopFactory();
        ByteBufAllocator byteBufAllocator = l4LoadBalancer().byteBufAllocator();

        // Build QUIC server codec with transport parameters from QuicConfiguration.
        // The channelInitializerFactory is called per-QuicChannel inside the ChannelInitializer,
        // NOT once at startup, because application protocol handlers (e.g. Http3ServerConnectionHandler)
        // are NOT @Sharable and hold per-connection state (QPACK, control streams).
        ChannelHandler quicServerCodec = new QuicServerCodecBuilder()
                .sslContext(quicSslContext)
                .maxIdleTimeout(quicConfig.maxIdleTimeoutMs(), TimeUnit.MILLISECONDS)
                .initialMaxData(quicConfig.initialMaxData())
                .initialMaxStreamDataBidirectionalLocal(quicConfig.initialMaxStreamDataBidiLocal())
                .initialMaxStreamDataBidirectionalRemote(quicConfig.initialMaxStreamDataBidiRemote())
                .initialMaxStreamDataUnidirectional(quicConfig.initialMaxStreamDataUni())
                .initialMaxStreamsBidirectional(quicConfig.initialMaxStreamsBidi())
                .initialMaxStreamsUnidirectional(quicConfig.initialMaxStreamsUni())
                .tokenHandler(customTokenHandler != null ? customTokenHandler : SecureQuicTokenHandler.INSTANCE)
                .handler(new ChannelInitializer<QuicChannel>() {
                    @Override
                    protected void initChannel(QuicChannel ch) {
                        activeQuicConnections.add(ch);
                        ch.pipeline().addLast(channelInitializerFactory.create(l4LoadBalancer()));
                    }
                })
                .build();

        WriteBufferWaterMark writeBufferWaterMark = new WriteBufferWaterMark(32 * 1024, 64 * 1024);

        Bootstrap bootstrap = new Bootstrap()
                .group(eventLoopFactory.parentGroup())
                .option(ChannelOption.ALLOCATOR, byteBufAllocator)
                .option(ChannelOption.RCVBUF_ALLOCATOR, configurationContext.transportConfiguration().recvByteBufAllocator())
                .option(ChannelOption.SO_SNDBUF, configurationContext.transportConfiguration().socketSendBufferSize())
                .option(ChannelOption.SO_RCVBUF, configurationContext.transportConfiguration().socketReceiveBufferSize())
                .option(ChannelOption.WRITE_BUFFER_WATER_MARK, writeBufferWaterMark)
                .option(ChannelOption.AUTO_READ, true)
                .option(ChannelOption.AUTO_CLOSE, false)
                .channelFactory(() -> {
                    TransportType transportType = configurationContext.transportConfiguration().transportType();
                    if (transportType == TransportType.IO_URING) {
                        IOUringDatagramChannel datagramChannel = new IOUringDatagramChannel();
                        datagramChannel.config().setOption(UnixChannelOption.SO_REUSEPORT, true);
                        return datagramChannel;
                    } else if (transportType == TransportType.EPOLL) {
                        EpollDatagramChannel datagramChannel = new EpollDatagramChannel();
                        EpollDatagramChannelConfig config = datagramChannel.config();
                        config.setEpollMode(EpollMode.EDGE_TRIGGERED);
                        config.setOption(UnixChannelOption.SO_REUSEPORT, true);
                        config.setUdpGro(true);
                        return datagramChannel;
                    } else {
                        return new NioDatagramChannel();
                    }
                })
                .handler(new ChannelInitializer<DatagramChannel>() {
                    @Override
                    protected void initChannel(DatagramChannel ch) {
                        ch.pipeline().addFirst(StandardEdgeNetworkMetricRecorder.INSTANCE);
                        ch.pipeline().addLast(quicServerCodec);
                    }
                });

        int bindRounds = 1;
        if (configurationContext.transportConfiguration().transportType().nativeTransport()) {
            bindRounds = configurationContext.eventLoopConfiguration().parentWorkers();
        }

        InetSocketAddress bindAddress = new InetSocketAddress(
                l4LoadBalancer().bindAddress().getAddress(), quicConfig.port());

        // Aggregate all bind futures: report success only when ALL sockets bind successfully.
        // With SO_REUSEPORT, missing sockets means silently dropped packets.
        AtomicInteger remainingBinds = new AtomicInteger(bindRounds);
        // Use array to allow capture in lambda (volatile semantics via AtomicInteger ordering)
        final Throwable[] firstFailure = {null};

        for (int i = 0; i < bindRounds; i++) {
            ChannelFuture channelFuture = bootstrap.bind(bindAddress);
            channelFutures.add(channelFuture);
            channelFuture.addListener(future -> {
                if (!future.isSuccess() && firstFailure[0] == null) {
                    firstFailure[0] = future.cause();
                }
                if (remainingBinds.decrementAndGet() == 0) {
                    if (firstFailure[0] != null) {
                        logger.error("Failed to start QUIC listener on {}", bindAddress, firstFailure[0]);
                        l4FrontListenerStartupEvent.markFailure(firstFailure[0]);
                    } else {
                        logger.info("QUIC listener started on {} with {} UDP socket(s)",
                                bindAddress, channelFutures.size());
                        l4FrontListenerStartupEvent.markSuccess(null);
                    }
                }
            });
        }

        l4LoadBalancer().eventStream().publish(l4FrontListenerStartupEvent);
        return l4FrontListenerStartupEvent;
    }

    @Override
    public L4FrontListenerStopTask stop() {
        L4FrontListenerStopTask l4FrontListenerStopEvent = new L4FrontListenerStopTask();

        if (channelFutures.isEmpty()) {
            l4FrontListenerStopEvent.markFailure(
                    new IllegalArgumentException("Listener has already stopped and cannot be stopped again."));
            return l4FrontListenerStopEvent;
        }

        channelFutures.forEach(channelFuture -> channelFuture.channel().close());

        channelFutures.get(channelFutures.size() - 1).channel().closeFuture().addListener(future -> {
            if (future.isSuccess()) {
                int activeCount = activeQuicConnections.size();
                if (activeCount > 0 && drainTimeoutSeconds > 0) {
                    logger.info("Draining {} active QUIC connections (timeout: {}s)",
                            activeCount, drainTimeoutSeconds);

                    GlobalEventExecutor.INSTANCE.schedule(() -> {
                        int remaining = activeQuicConnections.size();
                        if (remaining > 0) {
                            logger.info("Drain timeout reached, forcefully closing {} remaining QUIC connections",
                                    remaining);
                            activeQuicConnections.close();
                        }
                        channelFutures.clear();
                        // Close clusters AFTER drain completes so in-flight requests retain routing
                        l4LoadBalancer().clusters().forEach((hostname, cluster) -> cluster.close());
                        l4FrontListenerStopEvent.markSuccess(null);
                    }, drainTimeoutSeconds, TimeUnit.SECONDS);
                } else {
                    activeQuicConnections.close().awaitUninterruptibly(5, TimeUnit.SECONDS);
                    channelFutures.clear();
                    l4LoadBalancer().clusters().forEach((hostname, cluster) -> cluster.close());
                    l4FrontListenerStopEvent.markSuccess(null);
                }
            } else {
                l4FrontListenerStopEvent.markFailure(future.cause());
            }
        });

        l4LoadBalancer().eventStream().publish(l4FrontListenerStopEvent);
        return l4FrontListenerStopEvent;
    }

    @Override
    public L4FrontListenerShutdownTask shutdown() {
        L4FrontListenerStopTask event = stop();
        L4FrontListenerShutdownTask shutdownEvent = new L4FrontListenerShutdownTask();

        event.future().whenCompleteAsync((_void, throwable) -> {
            l4LoadBalancer().removeClusters();
            // Wait for EventLoopGroups to finish before marking shutdown complete
            l4LoadBalancer().eventLoopFactory().parentGroup().shutdownGracefully()
                    .addListener(f -> {
                        l4LoadBalancer().eventLoopFactory().childGroup().shutdownGracefully()
                                .addListener(f2 -> shutdownEvent.markSuccess());
                    });
        });

        return shutdownEvent;
    }
}
