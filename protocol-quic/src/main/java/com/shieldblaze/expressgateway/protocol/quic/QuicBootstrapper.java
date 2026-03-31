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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.socket.DatagramChannel;
import io.netty.handler.codec.quic.QuicChannel;
import io.netty.handler.codec.quic.QuicClientCodecBuilder;
import io.netty.handler.codec.quic.QuicSslContext;
import io.netty.handler.codec.quic.QuicSslContextBuilder;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.net.ssl.TrustManagerFactory;
import java.net.InetSocketAddress;
import java.security.KeyStore;
import java.util.concurrent.TimeUnit;
import java.util.function.Supplier;

/**
 * Creates backend QUIC connections to downstream servers.
 *
 * <p>This bootstrapper establishes QUIC connections (RFC 9000) with a configurable
 * application protocol. Unlike TCP-based bootstrappers, this class works with UDP
 * datagrams and the QUIC transport layer:</p>
 *
 * <ul>
 *   <li>Uses {@link QuicSslContextBuilder} for QUIC-TLS (RFC 9001)</li>
 *   <li>Configures QUIC transport parameters from {@link QuicConfiguration}</li>
 *   <li>The resulting {@link QuicChannel} is set on the {@link QuicConnection}</li>
 * </ul>
 *
 * @param <C> the connection type, must extend {@link QuicConnection}
 */
public class QuicBootstrapper<C extends QuicConnection> {

    private static final Logger logger = LogManager.getLogger(QuicBootstrapper.class);

    private final ConfigurationContext configurationContext;
    private final QuicConfiguration quicConfiguration;
    private final EventLoopGroup eventLoopGroup;
    private final ByteBufAllocator byteBufAllocator;
    private final String[] applicationProtocols;
    // Cached — QuicSslContext is thread-safe and expensive to build (native BoringSSL alloc).
    private final QuicSslContext quicSslContext;

    /**
     * Create a new QuicBootstrapper.
     *
     * @param configurationContext   the configuration context
     * @param eventLoopGroup         the event loop group for backend connections
     * @param byteBufAllocator       the allocator for buffers
     * @param applicationProtocols   ALPN protocols (e.g., "h3" for HTTP/3)
     */
    public QuicBootstrapper(ConfigurationContext configurationContext,
                            EventLoopGroup eventLoopGroup,
                            ByteBufAllocator byteBufAllocator,
                            String... applicationProtocols) {
        this.configurationContext = configurationContext;
        this.quicConfiguration = configurationContext.quicConfiguration();
        this.eventLoopGroup = eventLoopGroup;
        this.byteBufAllocator = byteBufAllocator;
        this.applicationProtocols = applicationProtocols;
        this.quicSslContext = buildQuicSslContext();
    }

    /**
     * Create a new QUIC backend connection to the given Node.
     *
     * @param node               the backend node to connect to
     * @param pool               the connection pool to register the new connection in
     * @param connectionFactory  supplier that creates the connection instance
     * @return the new connection (may still be handshaking)
     */
    public C connect(Node node, QuicConnectionPool<C> pool, Supplier<C> connectionFactory) {
        C connection = connectionFactory.get();

        ChannelHandler quicClientCodec = new QuicClientCodecBuilder()
                .sslContext(quicSslContext)
                .maxIdleTimeout(quicConfiguration.maxIdleTimeoutMs(), TimeUnit.MILLISECONDS)
                .initialMaxData(quicConfiguration.initialMaxData())
                .initialMaxStreamDataBidirectionalLocal(quicConfiguration.initialMaxStreamDataBidiLocal())
                .initialMaxStreamDataBidirectionalRemote(quicConfiguration.initialMaxStreamDataBidiRemote())
                .initialMaxStreamDataUnidirectional(quicConfiguration.initialMaxStreamDataUni())
                .initialMaxStreamsBidirectional(quicConfiguration.initialMaxStreamsBidi())
                .initialMaxStreamsUnidirectional(quicConfiguration.initialMaxStreamsUni())
                .build();

        Bootstrap udpBootstrap = BootstrapFactory.udp(configurationContext, eventLoopGroup, byteBufAllocator);
        udpBootstrap.handler(new ChannelInitializer<DatagramChannel>() {
            @Override
            protected void initChannel(DatagramChannel ch) {
                ch.pipeline().addFirst(new NodeBytesTracker(node));
                ch.pipeline().addLast(quicClientCodec);
            }
        });

        ChannelFuture bindFuture = udpBootstrap.bind(0);

        bindFuture.addListener(future -> {
            if (!future.isSuccess()) {
                logger.error("Failed to bind UDP channel for QUIC connection to {}: {}",
                        node.socketAddress(), future.cause().getMessage());
                pool.evict(connection);
                connection.close();
                return;
            }

            DatagramChannel datagramChannel = (DatagramChannel) bindFuture.channel();
            InetSocketAddress backendAddress = node.socketAddress();

            QuicChannel.newBootstrap(datagramChannel)
                    .remoteAddress(backendAddress)
                    .connect()
                    .addListener(connectFuture -> {
                        if (connectFuture.isSuccess()) {
                            @SuppressWarnings("unchecked")
                            io.netty.util.concurrent.Future<QuicChannel> qcFuture =
                                    (io.netty.util.concurrent.Future<QuicChannel>) connectFuture;
                            QuicChannel quicChannel = qcFuture.getNow();

                            connection.quicChannel(quicChannel);

                            quicChannel.closeFuture().addListener(closeFuture -> {
                                pool.evict(connection);
                                logger.debug("QUIC connection to {} closed", node.socketAddress());
                            });

                            logger.debug("QUIC connection established to {}", node.socketAddress());
                        } else {
                            logger.error("QUIC handshake failed to {}: {}",
                                    node.socketAddress(),
                                    connectFuture.cause() != null ? connectFuture.cause().getMessage() : "unknown");
                            // Evict the zombie connection from pool before closing resources.
                            // Without this, the pool holds a dead connection with null QuicChannel
                            // in INITIALIZED state, causing acquire() to return it forever (livelock).
                            pool.evict(connection);
                            connection.close();
                            datagramChannel.close();
                        }
                    });
        });

        connection.init(bindFuture);

        // Atomic register to prevent TOCTOU race where concurrent threads
        // all pass canCreateConnection() and exceed maxPerNode.
        if (!pool.tryRegister(node, connection)) {
            logger.debug("Pool at capacity for {}, closing excess QUIC connection", node.socketAddress());
            connection.close();
            return null;
        }

        return connection;
    }

    /**
     * Build the QUIC-TLS context for client-side connections.
     */
    private QuicSslContext buildQuicSslContext() {
        QuicSslContextBuilder builder = QuicSslContextBuilder.forClient();

        if (applicationProtocols.length > 0) {
            builder.applicationProtocols(applicationProtocols);
        }

        if (configurationContext.tlsClientConfiguration().enabled()
                && configurationContext.tlsClientConfiguration().acceptAllCerts()) {
            builder.trustManager(InsecureTrustManagerFactory.INSTANCE);
            logger.warn("QUIC-TLS using insecure trust manager (acceptAllCerts=true). "
                    + "This MUST NOT be used in production.");
        } else {
            try {
                TrustManagerFactory tmf = TrustManagerFactory.getInstance(
                        TrustManagerFactory.getDefaultAlgorithm());
                tmf.init((KeyStore) null);
                builder.trustManager(tmf);
            } catch (Exception e) {
                logger.error("Failed to initialize default TrustManagerFactory for QUIC-TLS, "
                        + "falling back to system default", e);
            }
        }

        if (quicConfiguration.zeroRttEnabled()) {
            builder.earlyData(true);
        }

        return builder.build();
    }
}
