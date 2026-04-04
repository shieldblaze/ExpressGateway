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
package com.shieldblaze.expressgateway.security;

import com.google.common.cache.Cache;
import com.google.common.cache.CacheBuilder;
import io.github.bucket4j.Bandwidth;
import io.github.bucket4j.Bucket;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.socket.DatagramPacket;
import io.netty.util.ReferenceCountUtil;
import lombok.extern.slf4j.Slf4j;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Objects;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.LongAdder;

/**
 * Token-bucket based packet rate limiter with per-source-IP tracking,
 * configurable burst handling, and response customization.
 *
 * <p>This handler sits in the Netty pipeline and enforces both a global
 * packet rate and per-source-IP rates. When a rate limit is exceeded,
 * the configured {@link OverLimitAction} determines the response.</p>
 *
 * <p>Counters use {@link LongAdder} for lock-free, low-contention tracking
 * on the hot path (every packet read).</p>
 */
@Slf4j
public final class PacketRateLimit extends ChannelDuplexHandler {

    /**
     * Action to take when a packet exceeds the rate limit.
     */
    public enum OverLimitAction {
        /** Silently drop the packet (release buffer, no response). */
        DROP,
        /** Close the channel after releasing the packet. */
        REJECT,
        /** Queue the packet for later delivery (not yet implemented -- falls back to DROP). */
        THROTTLE
    }

    private static final int DEFAULT_MAX_PER_IP_ENTRIES = 50_000;

    private final Bucket globalBucket;
    private final int perIpPackets;
    private final Duration perIpDuration;
    private final int perIpBurst;
    private final Cache<String, Bucket> perIpBuckets;
    private final OverLimitAction overLimitAction;

    private final LongAdder acceptedPackets = new LongAdder();
    private final LongAdder droppedPackets = new LongAdder();

    /**
     * Create a new {@link PacketRateLimit} Instance with only global rate limiting.
     * This is the backward-compatible constructor.
     *
     * @param packet   Number of packets for Rate-Limit, must be greater than 0
     * @param duration {@link Duration} of Rate-Limit, must not be null
     */
    public PacketRateLimit(int packet, Duration duration) {
        this(packet, duration, 0, null, 0, OverLimitAction.DROP, DEFAULT_MAX_PER_IP_ENTRIES);
    }

    /**
     * Create a new {@link PacketRateLimit} Instance with global and per-IP rate limiting.
     *
     * @param globalPackets    Global packet rate limit (must be > 0)
     * @param globalDuration   Global rate window
     * @param perIpPackets     Per-IP packet rate limit (0 to disable)
     * @param perIpDuration    Per-IP rate window (null to disable)
     * @param perIpBurst       Additional burst capacity per IP (0 for no burst)
     * @param action           What to do when rate limit is exceeded
     * @param maxPerIpEntries  Maximum number of tracked source IPs
     */
    public PacketRateLimit(int globalPackets, Duration globalDuration,
                           int perIpPackets, Duration perIpDuration,
                           int perIpBurst, OverLimitAction action,
                           int maxPerIpEntries) {
        if (globalPackets <= 0) {
            throw new IllegalArgumentException("globalPackets must be > 0, was: " + globalPackets);
        }
        Objects.requireNonNull(globalDuration, "globalDuration");
        if (globalDuration.isZero() || globalDuration.isNegative()) {
            throw new IllegalArgumentException("globalDuration must be positive");
        }

        Bandwidth globalLimit = Bandwidth.simple(globalPackets, globalDuration);
        this.globalBucket = Bucket.builder().addLimit(globalLimit).withNanosecondPrecision().build();

        this.perIpPackets = perIpPackets;
        this.perIpDuration = perIpDuration;
        this.perIpBurst = Math.max(0, perIpBurst);
        this.overLimitAction = action != null ? action : OverLimitAction.DROP;

        this.perIpBuckets = CacheBuilder.newBuilder()
                .maximumSize(maxPerIpEntries)
                .expireAfterAccess(Duration.ofMinutes(30))
                .build();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        // 1. Per-IP rate limit check FIRST to avoid wasting global tokens
        // when a single source is flooding. The global bucket is a shared
        // resource; consuming from it before rejecting on per-IP wastes
        // capacity that legitimate clients could use.
        if (perIpPackets > 0 && perIpDuration != null) {
            String sourceKey = extractSourceKey(ctx, msg);
            if (sourceKey != null) {
                Bucket ipBucket;
                try {
                    ipBucket = perIpBuckets.get(sourceKey, () -> {
                        long capacity = perIpPackets + perIpBurst;
                        Bandwidth limit = Bandwidth.simple(capacity, perIpDuration);
                        return Bucket.builder().addLimit(limit).withNanosecondPrecision().build();
                    });
                } catch (ExecutionException e) {
                    handleOverLimit(ctx, msg, "per-IP (bucket creation failed)");
                    return;
                }
                if (!ipBucket.tryConsume(1)) {
                    handleOverLimit(ctx, msg, "per-IP");
                    return;
                }
            }
        }

        // 2. Global rate limit check (only reached if per-IP passed)
        if (!globalBucket.tryConsume(1)) {
            handleOverLimit(ctx, msg, "global");
            return;
        }

        acceptedPackets.increment();
        super.channelRead(ctx, msg);
    }

    private void handleOverLimit(ChannelHandlerContext ctx, Object msg, String source) {
        droppedPackets.increment();

        if (msg instanceof DatagramPacket datagramPacket) {
            log.debug("Rate-Limit ({}) exceeded, denying packet from {}", source, datagramPacket.sender());
        } else {
            log.debug("Rate-Limit ({}) exceeded, denying packet from {}", source, ctx.channel().remoteAddress());
        }

        ReferenceCountUtil.release(msg);

        if (overLimitAction == OverLimitAction.REJECT) {
            ctx.close();
        }
    }

    private String extractSourceKey(ChannelHandlerContext ctx, Object msg) {
        if (msg instanceof DatagramPacket datagramPacket) {
            InetSocketAddress sender = datagramPacket.sender();
            if (sender != null) {
                return sender.getAddress().getHostAddress();
            }
        }
        var remoteAddr = ctx.channel().remoteAddress();
        if (remoteAddr instanceof InetSocketAddress inet) {
            return inet.getAddress().getHostAddress();
        }
        return null;
    }

    /**
     * Returns the total number of accepted packets.
     */
    public long acceptedPackets() {
        return acceptedPackets.sum();
    }

    /**
     * Returns the total number of dropped packets.
     */
    public long droppedPackets() {
        return droppedPackets.sum();
    }

    /**
     * Returns the number of tracked per-IP buckets.
     */
    public long trackedIpCount() {
        return perIpBuckets.size();
    }
}
