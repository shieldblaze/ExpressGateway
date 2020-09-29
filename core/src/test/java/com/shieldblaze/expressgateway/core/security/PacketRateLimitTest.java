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
