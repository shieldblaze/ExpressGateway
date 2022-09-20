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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import com.shieldblaze.expressgateway.metrics.CPU;
import com.shieldblaze.expressgateway.metrics.EdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.metrics.Memory;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Disabled;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ScaleMonitorTest {

    private static final EventStream eventStream = new EventStream();
    private static ScaleMonitor scaleMonitor;
    private static DummyAutoscaling dummyAutoscaling;

    @BeforeAll
    static void setup() {
        dummyAutoscaling = new DummyAutoscaling(eventStream, null);

        scaleMonitor = new ScaleMonitor(dummyAutoscaling, new CPU(), new Memory(), EdgeNetworkMetricRecorder.INSTANCE);
    }

    @AfterAll
    static void shutdown() {
        scaleMonitor.close();
        eventStream.close();
    }

    @Test
    @Disabled("needs revisit")
    void testScaleOutDueToPackets() {
        assertFalse(dummyAutoscaling.isScaleOut());

        EmbeddedChannel channel = new EmbeddedChannel(EdgeNetworkMetricRecorder.INSTANCE);

        for (int i = 0; i < 100; i++) {
            ByteBuf byteBuf = Unpooled.buffer();
            byteBuf.writeZero(100_000);

            channel.writeInbound(byteBuf);
            assertNotNull(channel.readInbound());

            byteBuf.release();
        }

        channel.close();
        assertTrue(dummyAutoscaling.isScaleOut());
    }
}
