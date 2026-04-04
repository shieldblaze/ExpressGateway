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
package com.shieldblaze.expressgateway.core.handlers;

import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.group.ChannelGroup;
import io.netty.channel.group.DefaultChannelGroup;
import io.netty.util.concurrent.GlobalEventExecutor;

import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * {@link ConnectionTracker} is a special handler that tracks number of active connections,
 * including per-IP connection counts for rate limiting (SEC-09).
 */
@ChannelHandler.Sharable
public final class ConnectionTracker extends ChannelInboundHandlerAdapter {

    private final AtomicInteger connections = new AtomicInteger();

    /**
     * HI-03: ChannelGroup tracking all active connections. Used by HTTPLoadBalancer
     * during graceful shutdown to iterate channels and initiate protocol-level draining
     * (H1: set Connection:close on in-flight responses; H2: send GOAWAY).
     * ChannelGroup automatically removes channels when they close.
     */
    private final ChannelGroup allChannels = new DefaultChannelGroup(GlobalEventExecutor.INSTANCE);

    /**
     * SEC-09: Per-IP active connection counts. Used to enforce max connections per source IP.
     * Key is {@link InetAddress} (not String) for efficient lookup and correct IPv6 handling.
     */
    private final ConcurrentHashMap<InetAddress, AtomicInteger> perIpConnections = new ConcurrentHashMap<>();

    /**
     * SEC-09: Maximum connections allowed per source IP. 0 means unlimited (disabled).
     */
    private volatile int maxConnectionsPerIp;

    /**
     * HI-04: Maximum total connections allowed. 0 means unlimited (disabled).
     * Under connection flood attacks, the proxy would otherwise accept connections
     * until the OS fd limit (ulimit -n), at which point ALL connections fail.
     * This limit provides a controlled rejection point with graceful handling.
     */
    private volatile int maxTotalConnections;

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        // HI-04: Enforce total connection limit before incrementing.
        // Check-then-increment is intentionally non-atomic — slightly exceeding
        // the limit under extreme burst is acceptable and avoids CAS contention.
        if (maxTotalConnections > 0 && connections.get() >= maxTotalConnections) {
            ctx.close();
            return;
        }

        allChannels.add(ctx.channel());
        increment();

        // SEC-09: Track per-IP connections and enforce limit
        if (maxConnectionsPerIp > 0) {
            InetAddress remoteIp = ((InetSocketAddress) ctx.channel().remoteAddress()).getAddress();
            AtomicInteger count = perIpConnections.computeIfAbsent(remoteIp, k -> new AtomicInteger());
            int current = count.incrementAndGet();
            if (current > maxConnectionsPerIp) {
                count.decrementAndGet();
                decrement();
                ctx.close();
                return;
            }
        }

        super.channelActive(ctx);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        decrement();

        // SEC-09: Decrement per-IP counter. Use compute() for atomic decrement-and-remove
        // to avoid race where a concurrent increment sees a zero-count entry between
        // decrementAndGet() and remove().
        if (maxConnectionsPerIp > 0 && ctx.channel().remoteAddress() instanceof InetSocketAddress isa) {
            InetAddress remoteIp = isa.getAddress();
            perIpConnections.computeIfPresent(remoteIp, (key, count) -> {
                int remaining = count.decrementAndGet();
                return remaining <= 0 ? null : count;
            });
        }

        super.channelInactive(ctx);
    }

    /**
     * Increment in the number of active connections
     */
    public void increment() {
        connections.incrementAndGet();
    }

    /**
     * Decrement in the number of active connections.
     * LOW-19: Guarded against going below zero to handle edge cases where
     * channelInactive fires without a corresponding channelActive.
     */
    public void decrement() {
        connections.updateAndGet(current -> Math.max(0, current - 1));
    }

    /**
     * Get the number of active connections
     */
    public int connections() {
        return connections.get();
    }

    /**
     * SEC-09: Set the maximum connections allowed per source IP.
     * @param max maximum connections per IP; 0 to disable
     */
    public void setMaxConnectionsPerIp(int max) {
        this.maxConnectionsPerIp = max;
    }

    /**
     * HI-04: Set the maximum total connections the proxy will accept.
     * Connections beyond this limit are immediately closed.
     * @param max maximum total connections; 0 to disable (unlimited)
     */
    public void setMaxTotalConnections(int max) {
        this.maxTotalConnections = max;
    }

    /**
     * SEC-09: Get per-IP connection count for a specific address.
     */
    public int connectionsForIp(InetAddress ip) {
        AtomicInteger count = perIpConnections.get(ip);
        return count != null ? count.get() : 0;
    }

    /**
     * HI-03: Get all active channels tracked by this handler.
     * Used by HTTPLoadBalancer during graceful shutdown to iterate channels
     * and initiate protocol-level draining.
     */
    public ChannelGroup allChannels() {
        return allChannels;
    }
}
