/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.security;

import io.netty.channel.ChannelHandler;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.util.internal.SocketUtils;
import org.junit.jupiter.api.Test;

import java.net.SocketAddress;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PacketRateLimitTest {

    @Test
    void test() throws InterruptedException {
        PacketRateLimit packetRateLimit = new PacketRateLimit(5, Duration.ofSeconds(10));

        EmbeddedChannel embeddedChannel = newEmbeddedInetChannel(packetRateLimit);
        for (int i = 0; i < 100; i++) {
            assertTrue(embeddedChannel.writeInbound("LOL" + i));
        }
        embeddedChannel.flushInbound();

        // Successfully Sent
        assertEquals("LOL0", embeddedChannel.readInbound());
        assertEquals("LOL1", embeddedChannel.readInbound());
        assertEquals("LOL2", embeddedChannel.readInbound());
        assertEquals("LOL3", embeddedChannel.readInbound());
        assertEquals("LOL4", embeddedChannel.readInbound());

        // Rate-Limited
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());

        Thread.sleep(15000L);

        for (int i = 0; i < 100; i++) {
            assertTrue(embeddedChannel.writeInbound("LOL" + i));
        }
        embeddedChannel.flushInbound();

        // Successfully Sent
        assertEquals("LOL0", embeddedChannel.readInbound());
        assertEquals("LOL1", embeddedChannel.readInbound());
        assertEquals("LOL2", embeddedChannel.readInbound());
        assertEquals("LOL3", embeddedChannel.readInbound());
        assertEquals("LOL4", embeddedChannel.readInbound());

        // Rate-Limited
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());
        assertNull(embeddedChannel.readInbound());

        assertTrue(embeddedChannel.close().isSuccess());
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
