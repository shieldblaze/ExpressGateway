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

import static org.junit.jupiter.api.Assertions.*;

class PacketRateLimitTest {

    @Test
    void globalRateLimitDropsExcessPackets() {
        PacketRateLimit limiter = new PacketRateLimit(5, Duration.ofSeconds(10));

        EmbeddedChannel ch = newEmbeddedInetChannel(limiter);

        // Send 10 string packets
        for (int i = 0; i < 10; i++) {
            ch.writeInbound("PKT" + i);
        }
        ch.flushInbound();

        // First 5 should pass
        assertEquals("PKT0", ch.readInbound());
        assertEquals("PKT1", ch.readInbound());
        assertEquals("PKT2", ch.readInbound());
        assertEquals("PKT3", ch.readInbound());
        assertEquals("PKT4", ch.readInbound());

        // Rest should be rate-limited
        assertNull(ch.readInbound());

        assertEquals(5, limiter.acceptedPackets());
        assertEquals(5, limiter.droppedPackets());

        ch.close();
    }

    @Test
    void constructorValidation() {
        assertThrows(IllegalArgumentException.class,
                () -> new PacketRateLimit(0, Duration.ofSeconds(1)));
        assertThrows(IllegalArgumentException.class,
                () -> new PacketRateLimit(-1, Duration.ofSeconds(1)));
        assertThrows(NullPointerException.class,
                () -> new PacketRateLimit(5, null));
    }

    @Test
    void perIpRateLimiting() {
        PacketRateLimit limiter = new PacketRateLimit(
                100, Duration.ofSeconds(10),          // global: generous
                3, Duration.ofSeconds(10),             // per-IP: 3/10s
                0, PacketRateLimit.OverLimitAction.DROP,
                1000
        );

        EmbeddedChannel ch = newEmbeddedInetChannel(limiter);

        // First 3 pass per-IP limit
        for (int i = 0; i < 3; i++) {
            ch.writeInbound("PKT" + i);
        }
        ch.flushInbound();

        assertEquals("PKT0", ch.readInbound());
        assertEquals("PKT1", ch.readInbound());
        assertEquals("PKT2", ch.readInbound());

        // 4th should be dropped by per-IP limit
        ch.writeInbound("PKT3");
        ch.flushInbound();
        assertNull(ch.readInbound());

        ch.close();
    }

    @Test
    void burstAllowsExtraPackets() {
        PacketRateLimit limiter = new PacketRateLimit(
                100, Duration.ofSeconds(10),
                3, Duration.ofSeconds(10),
                2, PacketRateLimit.OverLimitAction.DROP, // burst = 2
                1000
        );

        EmbeddedChannel ch = newEmbeddedInetChannel(limiter);

        // With burst=2, effective capacity = 3+2 = 5
        for (int i = 0; i < 5; i++) {
            ch.writeInbound("PKT" + i);
        }
        ch.flushInbound();

        for (int i = 0; i < 5; i++) {
            assertEquals("PKT" + i, ch.readInbound());
        }

        // 6th should be dropped
        ch.writeInbound("PKT5");
        ch.flushInbound();
        assertNull(ch.readInbound());

        ch.close();
    }

    private static EmbeddedChannel newEmbeddedInetChannel(ChannelHandler... handlers) {
        return new EmbeddedChannel(handlers) {
            @Override
            protected SocketAddress remoteAddress0() {
                return isActive() ? SocketUtils.socketAddress("127.0.0.1", 5421) : null;
            }
        };
    }
}
