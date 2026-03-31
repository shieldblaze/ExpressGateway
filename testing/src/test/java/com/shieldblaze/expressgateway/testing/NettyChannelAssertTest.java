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
package com.shieldblaze.expressgateway.testing;

import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.HttpServerCodec;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NettyChannelAssertTest {

    @Test
    void activeChannel() {
        EmbeddedChannel ch = new EmbeddedChannel();
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).isActive().isOpen().isWritable());
        ch.close();
    }

    @Test
    void inactiveChannelFails() {
        EmbeddedChannel ch = new EmbeddedChannel();
        ch.close();
        assertThrows(AssertionError.class, () -> NettyChannelAssert.assertThat(ch).isActive());
    }

    @Test
    void hasHandlerByName() {
        EmbeddedChannel ch = new EmbeddedChannel();
        ch.pipeline().addLast("codec", new HttpServerCodec());
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).hasHandler("codec"));
        ch.close();
    }

    @Test
    void missingHandlerFails() {
        EmbeddedChannel ch = new EmbeddedChannel();
        assertThrows(AssertionError.class, () -> NettyChannelAssert.assertThat(ch).hasHandler("nonexistent"));
        ch.close();
    }

    @Test
    void hasHandlerOfType() {
        EmbeddedChannel ch = new EmbeddedChannel(new HttpServerCodec());
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).hasHandlerOfType(HttpServerCodec.class));
        ch.close();
    }

    @Test
    void doesNotHaveHandler() {
        EmbeddedChannel ch = new EmbeddedChannel();
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).doesNotHaveHandler("ssl"));
        ch.close();
    }

    @Test
    void embeddedChannelOutboundMessages() {
        EmbeddedChannel ch = new EmbeddedChannel();
        // No outbound messages yet
        assertThrows(AssertionError.class, () -> NettyChannelAssert.assertThat(ch).hasOutboundMessages());

        ch.writeOutbound(Unpooled.wrappedBuffer("test".getBytes()));
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).hasOutboundMessages());
        ch.readOutbound(); // drain
        ch.close();
    }

    @Test
    void closedChannel() {
        EmbeddedChannel ch = new EmbeddedChannel();
        ch.close();
        assertDoesNotThrow(() -> NettyChannelAssert.assertThat(ch).isInactive().isClosed());
    }
}
