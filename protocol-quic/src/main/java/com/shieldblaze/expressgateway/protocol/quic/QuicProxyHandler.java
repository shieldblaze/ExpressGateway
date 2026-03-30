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
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.common.map.EntryRemovedListener;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;
import com.shieldblaze.expressgateway.configuration.quic.QuicConfiguration;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.DatagramPacket;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Native QUIC proxy handler for L4 (transport-level) QUIC datagram forwarding.
 *
 * <p>This handler operates at the UDP datagram layer without decoding HTTP/3 frames.
 * It receives incoming {@link DatagramPacket}s from the frontend, selects a backend
 * {@link Node} via load balancing, and forwards the raw QUIC datagrams bidirectionally.
 * This enables transparent QUIC proxying where the proxy does not terminate TLS
 * or inspect application-layer content.</p>
 *
 * <h3>CID-Based Routing (RFC 9000 Section 5.1, 9)</h3>
 * <p>When CID-based routing is enabled, the handler extracts the Destination Connection ID
 * (DCID) from each incoming QUIC packet header and uses it as the PRIMARY session lookup key.
 * This supports QUIC connection migration: when a client changes its source address (e.g.,
 * WiFi to cellular), the CID stays the same, so the proxy continues routing to the same
 * backend session. Address-based lookup is used as a FALLBACK for cases where the CID
 * cannot be extracted (e.g., Short Header with unknown CID length from a new source).</p>
 *
 * <h3>Session Tracking</h3>
 * <p>UDP is connectionless. Sessions are synthetic mappings from client address to backend
 * channel, managed via {@link SelfExpiringMap} with a configurable idle timeout. When a
 * session expires, the backend channel is closed and the connection tracker is decremented.
 * The CID session map ({@link QuicCidSessionMap}) maintains a parallel index keyed by DCID
 * for fast CID-based lookups with its own idle eviction.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>This handler is {@link ChannelHandler.Sharable} and safe for use across multiple
 * frontend DatagramChannels (e.g., SO_REUSEPORT multi-bind). The session map uses
 * {@link ConcurrentHashMap} backing. Load balancing decisions and session lookups are
 * lock-free.</p>
 */
@ChannelHandler.Sharable
public final class QuicProxyHandler extends ChannelInboundHandlerAdapter
        implements EntryRemovedListener<QuicBackendSession> {

    private static final Logger logger = LogManager.getLogger(QuicProxyHandler.class);

    /**
     * Default idle timeout for QUIC proxy sessions. Used as fallback when
     * QuicConfiguration is not available. Aligns with QUIC max_idle_timeout defaults.
     */
    private static final Duration DEFAULT_QUIC_SESSION_IDLE_TIMEOUT = Duration.ofSeconds(30);

    private final L4LoadBalancer l4LoadBalancer;
    private final Map<InetSocketAddress, QuicBackendSession> sessionMap;
    private final StandardEdgeNetworkMetricRecorder metricRecorder;

    /**
     * CID-based session index. Maps QUIC Destination Connection IDs to backend sessions,
     * enabling connection migration support. Null when CID routing is disabled.
     */
    private final QuicCidSessionMap cidSessionMap;

    /**
     * Whether CID-based routing is enabled for this handler instance.
     */
    private final boolean cidRoutingEnabled;

    /**
     * Create a new {@link QuicProxyHandler}.
     *
     * @param l4LoadBalancer the load balancer providing cluster and event loop resources
     */
    public QuicProxyHandler(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        this.metricRecorder = StandardEdgeNetworkMetricRecorder.INSTANCE;

        // Read session timeout and CID routing flag from QuicConfiguration.
        QuicConfiguration quicConfig = l4LoadBalancer.configurationContext().quicConfiguration();
        Duration sessionIdleTimeout;
        if (quicConfig != null && quicConfig.validated()) {
            sessionIdleTimeout = Duration.ofSeconds(quicConfig.quicProxySessionIdleTimeoutSeconds());
            this.cidRoutingEnabled = quicConfig.cidBasedRoutingEnabled();
        } else {
            sessionIdleTimeout = DEFAULT_QUIC_SESSION_IDLE_TIMEOUT;
            this.cidRoutingEnabled = true;
        }

        // Session map with self-expiring entries. When a session expires (no packets
        // within the idle timeout), the backend channel is closed via the
        // EntryRemovedListener callback.
        this.sessionMap = new SelfExpiringMap<>(
                new ConcurrentHashMap<>(),
                sessionIdleTimeout,
                true,
                this
        );

        // CID session map for connection migration support.
        if (cidRoutingEnabled) {
            this.cidSessionMap = new QuicCidSessionMap(sessionIdleTimeout);
        } else {
            this.cidSessionMap = null;
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (!(msg instanceof DatagramPacket datagramPacket)) {
            ReferenceCountUtil.safeRelease(msg);
            return;
        }

        try {
            InetSocketAddress sender = datagramPacket.sender();
            ByteBuf content = datagramPacket.content();
            QuicBackendSession session = null;
            byte[] dcid = null;
            int dcidLength = -1;

            // === CID-based lookup (PRIMARY) ===
            if (cidRoutingEnabled && content.readableBytes() > 0) {
                if (QuicPacketParser.isLongHeader(content)) {
                    // Long Header: DCID length is encoded in the packet.
                    dcidLength = QuicPacketParser.extractDcidLength(content);
                    dcid = QuicPacketParser.extractDCID(content, -1);
                } else {
                    // Short Header: DCID length is NOT in the packet.
                    // Strategy 1: Try address-based lookup to get a known session,
                    // which tells us the expected CID length.
                    QuicBackendSession addrSession = sessionMap.get(sender);
                    if (addrSession != null) {
                        // We have an address match. Use the known CID lengths to
                        // extract and verify the DCID.
                        int[] lengths = cidSessionMap.knownCidLengths();
                        for (int len : lengths) {
                            byte[] candidateDcid = QuicPacketParser.extractDCID(content, len);
                            if (candidateDcid != null) {
                                QuicBackendSession cidMatch = cidSessionMap.get(candidateDcid);
                                if (cidMatch != null) {
                                    session = cidMatch;
                                    dcid = candidateDcid;
                                    dcidLength = len;
                                    break;
                                }
                            }
                        }
                        // If CID lookup failed but address matched, use address session.
                        if (session == null) {
                            session = addrSession;
                        }
                    } else {
                        // Strategy 2: No address match (possible migration).
                        // Probe known CID lengths from the CID map.
                        int[] lengths = cidSessionMap.knownCidLengths();
                        for (int len : lengths) {
                            byte[] candidateDcid = QuicPacketParser.extractDCID(content, len);
                            if (candidateDcid != null) {
                                QuicBackendSession cidMatch = cidSessionMap.get(candidateDcid);
                                if (cidMatch != null) {
                                    session = cidMatch;
                                    dcid = candidateDcid;
                                    dcidLength = len;
                                    if (logger.isDebugEnabled()) {
                                        logger.debug("CID-based migration detected: client {} " +
                                                "routed via DCID to existing backend session", sender);
                                    }
                                    // Update address map for this migrated client.
                                    sessionMap.put(sender, session);
                                    break;
                                }
                            }
                        }
                    }
                }

                // If we extracted a DCID from a Long Header but haven't resolved a session yet,
                // try looking it up in the CID map.
                if (session == null && dcid != null) {
                    session = cidSessionMap.get(dcid);
                    if (session != null && logger.isDebugEnabled()) {
                        logger.debug("CID-based lookup matched Long Header DCID for client {}", sender);
                    }
                }
            }

            // === Address-based lookup (FALLBACK) ===
            if (session == null) {
                session = sessionMap.get(sender);
            }

            // === No existing session: create new ===
            if (session == null) {
                session = createSession(ctx.channel(), sender, dcid, dcidLength);
                if (session == null) {
                    // No backend available -- drop the packet.
                    ReferenceCountUtil.safeRelease(datagramPacket);
                    return;
                }
                sessionMap.put(sender, session);
                l4LoadBalancer.connectionTracker().increment();
            }

            // Forward the raw QUIC datagram content to the backend.
            // retain() because the DatagramPacket will be released after this method.
            session.writeAndFlush(content.retain());
        } catch (Exception e) {
            ReferenceCountUtil.safeRelease(datagramPacket);
            logger.error("Error processing QUIC proxy packet", e);
        }
    }

    /**
     * Create a new backend session by selecting a Node and connecting a UDP channel.
     *
     * @param frontendChannel the frontend DatagramChannel
     * @param clientAddress   the client's source address
     * @param dcid            the DCID extracted from the initial packet, or null
     * @param dcidLength      the DCID length, or -1 if unknown
     * @return the new session, or null if no backend is available
     */
    private QuicBackendSession createSession(Channel frontendChannel, InetSocketAddress clientAddress,
                                              byte[] dcid, int dcidLength) {
        try {
            Node node = l4LoadBalancer.defaultCluster()
                    .nextNode(new L4Request(clientAddress))
                    .node();

            Bootstrap bootstrap = BootstrapFactory.udp(
                    l4LoadBalancer.configurationContext(),
                    l4LoadBalancer.eventLoopFactory().childGroup(),
                    l4LoadBalancer.byteBufAllocator()
            );

            QuicBackendSession session = new QuicBackendSession(node);

            bootstrap.handler(new ChannelInitializer<>() {
                @Override
                protected void initChannel(Channel ch) {
                    ChannelPipeline pipeline = ch.pipeline();
                    // NodeBytesTracker must be first to track all bytes in/out on the backend Node.
                    pipeline.addFirst(new NodeBytesTracker(node));
                    pipeline.addLast(StandardEdgeNetworkMetricRecorder.INSTANCE);
                    pipeline.addLast(new QuicProxyBackendHandler(frontendChannel, clientAddress, session, node));
                }
            });

            ChannelFuture channelFuture = bootstrap.connect(node.socketAddress());
            session.init(channelFuture);
            node.addConnection(session);

            // Register in CID session map if we have a valid DCID.
            if (cidRoutingEnabled && dcid != null && dcid.length > 0) {
                int effectiveCidLength = dcidLength > 0 ? dcidLength : dcid.length;
                cidSessionMap.put(dcid, session, effectiveCidLength);
                if (logger.isDebugEnabled()) {
                    logger.debug("Registered CID mapping for client {} (CID length: {})",
                            clientAddress, effectiveCidLength);
                }
            }

            return session;
        } catch (Exception e) {
            logger.error("Failed to create QUIC proxy backend session for client {}", clientAddress, e);
            metricRecorder.recordConnectionError();
            return null;
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        logger.info("QUIC proxy frontend channel inactive, closing all backend sessions");
        ((Closeable) sessionMap).close();
        sessionMap.forEach((address, session) -> session.close());
        sessionMap.clear();
        if (cidSessionMap != null) {
            cidSessionMap.close();
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Error in QUIC proxy handler", cause);
    }

    /**
     * Called when a session expires from the {@link SelfExpiringMap}.
     * Closes the backend channel and decrements the connection tracker.
     */
    @Override
    public void removed(QuicBackendSession value) {
        l4LoadBalancer.connectionTracker().decrement();
        value.close();
        // Note: CID entries are evicted independently by QuicCidSessionMap's own
        // idle timeout sweep. This avoids needing to track which CIDs map to which
        // address-based session (a QUIC connection may have multiple CIDs via
        // NEW_CONNECTION_ID frames, and we only see the DCID per packet).
    }

    /**
     * Inbound handler on the backend UDP channel. Receives response datagrams from
     * the backend and forwards them back to the original client via the frontend channel.
     */
    private static final class QuicProxyBackendHandler extends ChannelInboundHandlerAdapter {

        private static final Logger logger = LogManager.getLogger(QuicProxyBackendHandler.class);

        private final Channel frontendChannel;
        private final InetSocketAddress clientAddress;
        private final QuicBackendSession session;
        private final Node node;

        QuicProxyBackendHandler(Channel frontendChannel, InetSocketAddress clientAddress,
                                QuicBackendSession session, Node node) {
            this.frontendChannel = frontendChannel;
            this.clientAddress = clientAddress;
            this.session = session;
            this.node = node;
        }

        @Override
        public void channelRead(ChannelHandlerContext ctx, Object msg) {
            if (msg instanceof DatagramPacket packet) {
                try {
                    frontendChannel.writeAndFlush(
                            new DatagramPacket(packet.content().retain(), clientAddress),
                            frontendChannel.voidPromise()
                    );
                } finally {
                    ReferenceCountUtil.safeRelease(packet);
                }
            } else if (msg instanceof ByteBuf byteBuf) {
                try {
                    frontendChannel.writeAndFlush(
                            new DatagramPacket(byteBuf.retain(), clientAddress),
                            frontendChannel.voidPromise()
                    );
                } finally {
                    ReferenceCountUtil.safeRelease(byteBuf);
                }
            } else {
                ReferenceCountUtil.safeRelease(msg);
            }
        }

        @Override
        public void channelInactive(ChannelHandlerContext ctx) {
            if (logger.isDebugEnabled()) {
                logger.debug("Backend channel inactive for client {}, node {}",
                        clientAddress, node.socketAddress());
            }
            session.close();
        }

        @Override
        public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
            logger.error("Error in QUIC proxy backend handler for client {}", clientAddress, cause);
            ctx.channel().close();
        }
    }
}
