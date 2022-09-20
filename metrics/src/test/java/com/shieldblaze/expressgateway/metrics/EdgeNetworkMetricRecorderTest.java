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
package com.shieldblaze.expressgateway.metrics;

import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class EdgeNetworkMetricRecorderTest {

    static EmbeddedChannel channel;

    @BeforeAll
    static void setup() {
        channel = new EmbeddedChannel(EdgeNetworkMetricRecorder.INSTANCE);
    }

    @AfterAll
    static void shutdown() {
        assertTrue(channel.close().isSuccess());
    }

    @Test
    void outboundTest() {
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.packetTX());

        // Write 1 packet
        assertTrue(channel.writeOutbound(Unpooled.EMPTY_BUFFER));
        assertEquals(Unpooled.EMPTY_BUFFER, channel.readOutbound());

        assertEquals(1, EdgeNetworkMetricRecorder.INSTANCE.packetTX());
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.packetTX());

        // Write 1 MB of Data
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.bandwidthTX());
        ByteBuf byteBuf = Unpooled.buffer().writeZero(1000000);
        assertTrue(channel.writeOutbound(byteBuf));
        assertEquals(byteBuf, channel.readOutbound());
        byteBuf.release();

        assertEquals(1000000, EdgeNetworkMetricRecorder.INSTANCE.bandwidthTX());
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.bandwidthTX());
    }

    @Test
    void inboundTest() {
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.packetRX());

        // Write 1 packet
        assertTrue(channel.writeInbound(Unpooled.EMPTY_BUFFER));
        assertEquals(Unpooled.EMPTY_BUFFER, channel.readInbound());

        assertEquals(1, EdgeNetworkMetricRecorder.INSTANCE.packetRX());
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.packetRX());

        // Write 1 MB of Data
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.bandwidthRX());
        ByteBuf byteBuf = Unpooled.buffer().writeZero(1000000);
        assertTrue(channel.writeInbound(byteBuf));
        assertEquals(byteBuf, channel.readInbound());
        byteBuf.release();

        assertEquals(1000000, EdgeNetworkMetricRecorder.INSTANCE.bandwidthRX());
        assertEquals(0, EdgeNetworkMetricRecorder.INSTANCE.bandwidthRX());
    }
}
