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

import java.util.Map;

import static org.junit.jupiter.api.Assertions.*;

class StandardEdgeNetworkMetricRecorderTest {

    static EmbeddedChannel channel;

    @BeforeAll
    static void setup() {
        channel = new EmbeddedChannel(StandardEdgeNetworkMetricRecorder.INSTANCE);
    }

    @AfterAll
    static void shutdown() {
        assertTrue(channel.close().isSuccess());
    }

    @Test
    void outboundTest() {
        long baseTX = StandardEdgeNetworkMetricRecorder.INSTANCE.packetTX();

        assertTrue(channel.writeOutbound(Unpooled.EMPTY_BUFFER));
        assertEquals(Unpooled.EMPTY_BUFFER, channel.readOutbound());

        assertEquals(baseTX + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.packetTX());
        // Monotonic: reading again returns same value
        assertEquals(baseTX + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.packetTX());

        long baseBW = StandardEdgeNetworkMetricRecorder.INSTANCE.bandwidthTX();
        ByteBuf byteBuf = Unpooled.buffer().writeZero(1000000);
        assertTrue(channel.writeOutbound(byteBuf));
        assertEquals(byteBuf, channel.readOutbound());
        byteBuf.release();

        assertEquals(baseBW + 1000000, StandardEdgeNetworkMetricRecorder.INSTANCE.bandwidthTX());
    }

    @Test
    void inboundTest() {
        long baseRX = StandardEdgeNetworkMetricRecorder.INSTANCE.packetRX();

        assertTrue(channel.writeInbound(Unpooled.EMPTY_BUFFER));
        assertEquals(Unpooled.EMPTY_BUFFER, channel.readInbound());

        assertEquals(baseRX + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.packetRX());

        long baseBW = StandardEdgeNetworkMetricRecorder.INSTANCE.bandwidthRX();
        ByteBuf byteBuf = Unpooled.buffer().writeZero(1000000);
        assertTrue(channel.writeInbound(byteBuf));
        assertEquals(byteBuf, channel.readInbound());
        byteBuf.release();

        assertEquals(baseBW + 1000000, StandardEdgeNetworkMetricRecorder.INSTANCE.bandwidthRX());
    }

    @Test
    void activeConnectionsTest() {
        long base = StandardEdgeNetworkMetricRecorder.INSTANCE.activeConnections();

        StandardEdgeNetworkMetricRecorder.INSTANCE.incrementActiveConnections();
        StandardEdgeNetworkMetricRecorder.INSTANCE.incrementActiveConnections();
        assertEquals(base + 2, StandardEdgeNetworkMetricRecorder.INSTANCE.activeConnections());

        StandardEdgeNetworkMetricRecorder.INSTANCE.decrementActiveConnections();
        assertEquals(base + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.activeConnections());

        StandardEdgeNetworkMetricRecorder.INSTANCE.decrementActiveConnections();
        assertEquals(base, StandardEdgeNetworkMetricRecorder.INSTANCE.activeConnections());
    }

    @Test
    void statusCodeTest() {
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordStatusCode(200);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordStatusCode(200);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordStatusCode(404);

        Map<Integer, Long> counts = StandardEdgeNetworkMetricRecorder.INSTANCE.statusCodeCounts();
        assertTrue(counts.get(200) >= 2);
        assertTrue(counts.get(404) >= 1);
    }

    @Test
    void latencyTest() {
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordLatency(10);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordLatency(20);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordLatency(30);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordLatency(100);

        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyCount() >= 4);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencySum() >= 160);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyMin() <= 10);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyMax() >= 100);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyP50() >= 0);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyP95() >= 0);
        assertTrue(StandardEdgeNetworkMetricRecorder.INSTANCE.latencyP99() >= 0);
    }

    @Test
    void connectionErrorTest() {
        long base = StandardEdgeNetworkMetricRecorder.INSTANCE.connectionErrors();
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordConnectionError();
        assertEquals(base + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.connectionErrors());
    }

    @Test
    void tlsErrorTest() {
        long base = StandardEdgeNetworkMetricRecorder.INSTANCE.tlsErrors();
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordTlsError();
        assertEquals(base + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.tlsErrors());
    }

    @Test
    void rateLimitRejectionTest() {
        long base = StandardEdgeNetworkMetricRecorder.INSTANCE.rateLimitRejections();
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordRateLimitRejection();
        assertEquals(base + 1, StandardEdgeNetworkMetricRecorder.INSTANCE.rateLimitRejections());
    }

    @Test
    void backendLatencyTest() {
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendLatency("backend-1", 50);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendLatency("backend-1", 100);

        Map<String, Long> latencies = StandardEdgeNetworkMetricRecorder.INSTANCE.backendLatencies();
        assertTrue(latencies.containsKey("backend-1"));
        assertTrue(latencies.get("backend-1") >= 50); // average of 50 and 100 = 75
    }

    @Test
    void protocolMetricsTest() {
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordProtocolBytes("HTTP/2", 1024);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordProtocolBytes("HTTP/2", 2048);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordProtocolRequest("HTTP/2");
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordProtocolRequest("TCP");

        Map<String, Long> bytes = StandardEdgeNetworkMetricRecorder.INSTANCE.protocolBytes();
        assertTrue(bytes.get("HTTP/2") >= 3072);

        Map<String, Long> requests = StandardEdgeNetworkMetricRecorder.INSTANCE.protocolRequests();
        assertTrue(requests.get("HTTP/2") >= 1);
        assertTrue(requests.get("TCP") >= 1);
    }

    @Test
    void backendConnectionsTest() {
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendConnections("backend-a", 5);
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendConnections("backend-b", 10);

        Map<String, Integer> conns = StandardEdgeNetworkMetricRecorder.INSTANCE.backendConnections();
        assertEquals(5, conns.get("backend-a"));
        assertEquals(10, conns.get("backend-b"));

        // Update is a gauge -- overwrites
        StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendConnections("backend-a", 3);
        conns = StandardEdgeNetworkMetricRecorder.INSTANCE.backendConnections();
        assertEquals(3, conns.get("backend-a"));
    }

    @SuppressWarnings("deprecation")
    @Test
    void resetIsNoOp() {
        long baseTX = StandardEdgeNetworkMetricRecorder.INSTANCE.packetTX();
        StandardEdgeNetworkMetricRecorder.INSTANCE.reset();
        // reset is deprecated no-op -- counters remain unchanged
        assertEquals(baseTX, StandardEdgeNetworkMetricRecorder.INSTANCE.packetTX());
    }
}
