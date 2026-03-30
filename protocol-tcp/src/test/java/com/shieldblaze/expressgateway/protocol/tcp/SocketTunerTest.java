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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.configuration.transport.TransportType;
import io.netty.channel.Channel;
import io.netty.channel.ChannelOption;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.SocketChannelConfig;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.mockito.ArgumentMatchers.any;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

/**
 * Tests for SocketTuner transport-specific socket option configuration.
 */
final class SocketTunerTest {

    @SuppressWarnings("unchecked")
    @Test
    void lowLatencyProfileSetsNoDelay() {
        SocketChannel channel = mock(SocketChannel.class);
        SocketChannelConfig config = mock(SocketChannelConfig.class);
        when(channel.config()).thenReturn(config);
        // setOption returns boolean in Netty 4.2.x -- Mockito returns false by default
        // which is fine since we only care about the verify calls

        SocketTuner.tune(channel, TransportType.NIO, SocketTuner.TuningProfile.LOW_LATENCY);

        verify(config).setOption(ChannelOption.TCP_NODELAY, true);
        verify(config).setOption(ChannelOption.SO_KEEPALIVE, true);
    }

    @SuppressWarnings("unchecked")
    @Test
    void throughputProfileEnablesNagle() {
        SocketChannel channel = mock(SocketChannel.class);
        SocketChannelConfig config = mock(SocketChannelConfig.class);
        when(channel.config()).thenReturn(config);

        SocketTuner.tune(channel, TransportType.NIO, SocketTuner.TuningProfile.THROUGHPUT);

        verify(config).setOption(ChannelOption.TCP_NODELAY, false);
        verify(config).setOption(ChannelOption.SO_KEEPALIVE, true);
    }

    @Test
    void nonSocketChannelIsIgnored() {
        Channel channel = mock(Channel.class); // Not a SocketChannel
        assertDoesNotThrow(() ->
                SocketTuner.tune(channel, TransportType.NIO, SocketTuner.TuningProfile.LOW_LATENCY));
    }

    @Test
    void tuningProfileRecordAccessors() {
        SocketTuner.TuningProfile profile = new SocketTuner.TuningProfile(true, false, true);
        assertNotNull(profile);
        assert profile.tcpNoDelay();
        assert !profile.keepAlive();
        assert profile.quickAck();
    }

    @Test
    void predefinedProfiles() {
        SocketTuner.TuningProfile low = SocketTuner.TuningProfile.LOW_LATENCY;
        assert low.tcpNoDelay();
        assert low.keepAlive();
        assert low.quickAck();

        SocketTuner.TuningProfile throughput = SocketTuner.TuningProfile.THROUGHPUT;
        assert !throughput.tcpNoDelay();
        assert throughput.keepAlive();
        assert !throughput.quickAck();
    }
}
