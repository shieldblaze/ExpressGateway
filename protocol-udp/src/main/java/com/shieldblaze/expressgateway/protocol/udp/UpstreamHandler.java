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
package com.shieldblaze.expressgateway.protocol.udp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.common.map.EntryRemovedListener;
import com.shieldblaze.expressgateway.common.map.SelfExpiringMap;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.socket.DatagramPacket;
import io.netty.util.ReferenceCountUtil;
import lombok.extern.log4j.Log4j2;

import java.io.Closeable;
import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.LongAdder;

@Log4j2
@ChannelHandler.Sharable
final class UpstreamHandler extends ChannelInboundHandlerAdapter implements EntryRemovedListener<UDPConnection> {

    // UDP-F3: Named constant for the default UDP session idle timeout. UDP is connectionless,
    // so "sessions" are synthetic mappings from client address to upstream connection.
    // Unlike TCP idle timeouts (which detect dead connections via keepalive probes), UDP
    // sessions should expire faster since there is no connection state to preserve.
    // This default is used when the transport configuration value is not explicitly set,
    // and is deliberately shorter than the TCP default (120s) to avoid accumulating stale
    // entries in the connection map at high packet rates.
    private static final Duration DEFAULT_UDP_SESSION_IDLE_TIMEOUT = Duration.ofSeconds(30);

    private final Map<InetSocketAddress, UDPConnection> connectionMap;
    private final L4LoadBalancer l4LoadBalancer;
    private final Bootstrapper bootstrapper;
    private final SessionRateLimiter rateLimiter;
    private final LongAdder totalPacketsForwarded = new LongAdder();
    private final LongAdder totalBytesForwarded = new LongAdder();
    private final LongAdder undeliverableCount = new LongAdder();

    UpstreamHandler(L4LoadBalancer l4LoadBalancer) {
        this(l4LoadBalancer, SessionRateLimiter.Config.DISABLED);
    }

    UpstreamHandler(L4LoadBalancer l4LoadBalancer, SessionRateLimiter.Config rateLimitConfig) {
        this.l4LoadBalancer = l4LoadBalancer;
        bootstrapper = new Bootstrapper(l4LoadBalancer, l4LoadBalancer.eventLoopFactory().childGroup(), l4LoadBalancer.byteBufAllocator());
        this.rateLimiter = new SessionRateLimiter(rateLimitConfig);

        // CRIT-04: Wire this UpstreamHandler as EntryRemovedListener via 4-arg constructor.
        // Use the UDP-specific 30-second idle timeout rather than the shared TCP/UDP
        // connectionIdleTimeout (which defaults to 120s for TCP). UDP sessions are
        // synthetic and should expire faster since there is no connection state to preserve.
        //
        // autoRenew=true: Reset TTL on get() so active UDP sessions (game traffic, VPN,
        // media streaming) survive as long as packets flow. Sessions idle for 30s are
        // expired by the DefaultCleaner background thread (runs every 2s). The map can
        // only grow as large as the number of unique sources sending packets within the
        // TTL window -- bounded by network capacity, not unbounded.
        connectionMap = new SelfExpiringMap<>(
                new ConcurrentHashMap<>(),
                DEFAULT_UDP_SESSION_IDLE_TIMEOUT,
                true,
                this
        );
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        // Process directly on the current EventLoop. The handler is @Sharable and the
        // connectionMap is backed by ConcurrentHashMap, so it's thread-safe.
        //
        // The previous implementation dispatched to childGroup().next() which round-robins
        // across child EventLoops. This caused two bugs:
        // 1. Different packets from the same client could race on connectionMap, creating
        //    duplicate UDPConnections for the same source address.
        // 2. The "already on child EventLoop" optimization checked inEventLoop() on a
        //    rotating EventLoop, so the same thread could be "in" one iteration and "not in"
        //    the next, causing inconsistent dispatch behavior.
        //
        // For UDP, the parent EventLoop handles both inbound reads and backend bootstrapping.
        // There is no child pipeline (unlike TCP's ServerBootstrap accepting connections).
        processPacket(ctx, msg);
    }

    private void processPacket(ChannelHandlerContext ctx, Object msg) {
        DatagramPacket datagramPacket = (DatagramPacket) msg;
        try {
            // Rate limiting check -- per source IP
            if (!rateLimiter.tryAcquire(datagramPacket.sender().getAddress())) {
                log.debug("Rate limited datagram from {}", datagramPacket.sender());
                ReferenceCountUtil.safeRelease(datagramPacket);
                return;
            }

            UDPConnection udpConnection = connectionMap.get(datagramPacket.sender());

            // If connection is null then we need to establish a new connection to the node.
            if (udpConnection == null) {
                try {
                    Node node = l4LoadBalancer.defaultCluster().nextNode(new L4Request(datagramPacket.sender())).node();
                    udpConnection = bootstrapper.newInit(ctx.channel(), node, datagramPacket.sender());
                    node.addConnection(udpConnection);
                    connectionMap.put(datagramPacket.sender(), udpConnection);
                    l4LoadBalancer.connectionTracker().increment();
                } catch (Exception e) {
                    // MED-18: Release the datagram content on failure to prevent ByteBuf leak
                    undeliverableCount.increment();
                    ReferenceCountUtil.safeRelease(datagramPacket);
                    return;
                }
            }

            // Track statistics
            totalPacketsForwarded.increment();
            totalBytesForwarded.add(datagramPacket.content().readableBytes());

            udpConnection.writeAndFlush(datagramPacket.content());
        } catch (Exception e) {
            // MED-18: Ensure message is released on any unexpected exception
            undeliverableCount.increment();
            ReferenceCountUtil.safeRelease(datagramPacket);
            log.error("Error processing UDP packet", e);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        log.info("Closing all UDP sessions ({} active)", connectionMap.size());

        // Close all backend connections first, then stop the cleaner and clear the map.
        // Order matters: closing connections before stopping the cleaner prevents the
        // cleaner from racing with our explicit cleanup and double-closing connections.
        connectionMap.forEach((socketAddress, udpConnection) -> {
            try {
                udpConnection.close();
            } catch (Exception e) {
                log.debug("Error closing UDP connection to {}", socketAddress, e);
            }
        });

        try {
            ((Closeable) connectionMap).close();
        } catch (Exception e) {
            log.debug("Error closing SelfExpiringMap cleaner", e);
        }
        connectionMap.clear();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        log.error("Caught Error at Upstream Handler", cause);
        ctx.channel().close();
    }

    @Override
    public void removed(UDPConnection value) {
        l4LoadBalancer.connectionTracker().decrement();
        value.close();
    }

    /**
     * Total packets successfully forwarded since creation.
     */
    long totalPacketsForwarded() {
        return totalPacketsForwarded.sum();
    }

    /**
     * Total bytes forwarded since creation.
     */
    long totalBytesForwarded() {
        return totalBytesForwarded.sum();
    }

    /**
     * Total undeliverable datagrams (backend connect failed, errors).
     */
    long undeliverableCount() {
        return undeliverableCount.sum();
    }

    /**
     * Access the rate limiter for monitoring/testing.
     */
    SessionRateLimiter rateLimiter() {
        return rateLimiter;
    }

    /**
     * Number of active sessions in the connection map.
     */
    int activeSessionCount() {
        return connectionMap.size();
    }
}
