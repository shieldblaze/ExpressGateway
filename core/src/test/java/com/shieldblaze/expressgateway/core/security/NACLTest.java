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

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class NACLTest {

    @Test
    void test() {
        NACL nacl = new NACL(1, Duration.ofMinutes(1));

        EmbeddedChannel ch1 = newEmbeddedInetChannel(nacl);
        assertTrue(ch1.isActive());
        assertTrue(ch1.close().isSuccess());

        EmbeddedChannel ch2 = newEmbeddedInetChannel(nacl);
        assertFalse(ch2.isActive());
        assertTrue(ch2.close().isSuccess());

        EmbeddedChannel ch3 = newEmbeddedInetChannel(nacl);
        assertFalse(ch3.isActive());
        assertTrue(ch3.close().isSuccess());

        EmbeddedChannel ch4 = newEmbeddedInetChannel(nacl);
        assertFalse(ch4.isActive());
        assertTrue(ch4.close().isSuccess());

        EmbeddedChannel ch5 = newEmbeddedInetChannel(nacl);
        assertFalse(ch5.isActive());
        assertTrue(ch5.close().isSuccess());
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
