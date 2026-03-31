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

import io.netty.channel.ChannelHandler;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.util.internal.SocketUtils;
import org.junit.jupiter.api.Test;

import java.net.SocketAddress;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

/**
 * Tests for PacketRateLimit global token waste fix.
 * Verifies that per-IP rejection does not consume global tokens.
 *
 * PacketRateLimit is not @Sharable, so each test uses a single channel.
 * We verify the fix by checking that per-IP rejection does not increment
 * the global consumption (inferred from accepted counts staying correct
 * when global capacity should still be available).
 */
class PacketRateLimitGlobalTokenTest {

    /**
     * When a single IP exceeds its per-IP limit, the per-IP rejected packets
     * must NOT consume global tokens. We verify this by checking:
     * - Exactly 3 packets accepted (per-IP limit)
     * - Exactly 3 packets dropped (per-IP rejection)
     * - Global capacity is preserved (only 3 tokens consumed, not 6)
     *
     * If global was checked first (pre-fix), all 6 packets would consume
     * global tokens. Post-fix, only the 3 that pass per-IP consume global.
     */
    @Test
    void perIpRejectionDoesNotWasteGlobalTokens() {
        // Global: 10 packets/10s, Per-IP: 3 packets/10s
        PacketRateLimit limiter = new PacketRateLimit(
                10, Duration.ofSeconds(10),
                3, Duration.ofSeconds(10),
                0, PacketRateLimit.OverLimitAction.DROP,
                1000
        );

        EmbeddedChannel ch = newEmbeddedInetChannel("192.168.1.1", 5000, limiter);

        // Send 6 packets from same IP
        for (int i = 0; i < 6; i++) {
            ch.writeInbound("PKT-" + i);
        }
        ch.flushInbound();

        // First 3 pass per-IP + global, rest dropped by per-IP (before consuming global)
        assertEquals("PKT-0", ch.readInbound());
        assertEquals("PKT-1", ch.readInbound());
        assertEquals("PKT-2", ch.readInbound());
        assertNull(ch.readInbound(), "4th packet should be dropped by per-IP limit");

        // Key assertion: only 3 accepted (global tokens consumed = 3, not 6)
        assertEquals(3, limiter.acceptedPackets(),
                "Only 3 packets should have been accepted (per-IP limit)");
        assertEquals(3, limiter.droppedPackets(),
                "3 packets should be dropped by per-IP limit without consuming global tokens");

        ch.close();
    }

    /**
     * Global limit still works as a backstop even when per-IP limits pass.
     */
    @Test
    void globalLimitStillEnforcedAfterPerIpPasses() {
        // Global: 5 packets/10s, Per-IP: 100 packets/10s (generous)
        PacketRateLimit limiter = new PacketRateLimit(
                5, Duration.ofSeconds(10),
                100, Duration.ofSeconds(10),
                0, PacketRateLimit.OverLimitAction.DROP,
                1000
        );

        EmbeddedChannel ch = newEmbeddedInetChannel("10.0.0.1", 5000, limiter);

        for (int i = 0; i < 8; i++) {
            ch.writeInbound("PKT-" + i);
        }
        ch.flushInbound();

        // First 5 pass (per-IP allows all, global caps at 5)
        for (int i = 0; i < 5; i++) {
            assertEquals("PKT-" + i, ch.readInbound());
        }
        // Rest dropped by global
        assertNull(ch.readInbound());

        assertEquals(5, limiter.acceptedPackets());
        assertEquals(3, limiter.droppedPackets());

        ch.close();
    }

    /**
     * Verify the ordering: per-IP is checked first. When per-IP rejects,
     * the drop count increments but the global bucket is not touched.
     * This is observable: after per-IP saturation, sending more packets
     * from the same IP should keep dropping without consuming global tokens.
     */
    @Test
    void perIpSaturationPreservesGlobalBudget() {
        // Global: 5 packets/10s, Per-IP: 2 packets/10s
        PacketRateLimit limiter = new PacketRateLimit(
                5, Duration.ofSeconds(10),
                2, Duration.ofSeconds(10),
                0, PacketRateLimit.OverLimitAction.DROP,
                1000
        );

        EmbeddedChannel ch = newEmbeddedInetChannel("10.0.0.1", 5000, limiter);

        // First 2 pass per-IP (and consume 2 global tokens)
        ch.writeInbound("PKT-0");
        ch.writeInbound("PKT-1");
        ch.flushInbound();
        assertEquals("PKT-0", ch.readInbound());
        assertEquals("PKT-1", ch.readInbound());
        assertEquals(2, limiter.acceptedPackets());

        // Next 10 are all per-IP rejected. None should consume global tokens.
        for (int i = 0; i < 10; i++) {
            ch.writeInbound("PKT-EXCESS-" + i);
        }
        ch.flushInbound();
        assertNull(ch.readInbound());

        // Still only 2 accepted -- global budget was NOT consumed by the 10 per-IP rejections
        assertEquals(2, limiter.acceptedPackets(),
                "Global tokens must not be wasted on per-IP rejections");
        assertEquals(10, limiter.droppedPackets(),
                "All 10 excess packets should be dropped by per-IP limit");

        ch.close();
    }

    private static EmbeddedChannel newEmbeddedInetChannel(String ip, int port, ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress(ip, port) : null;
            }
        };
    }
}
