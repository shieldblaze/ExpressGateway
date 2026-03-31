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

import io.micrometer.core.instrument.Meter;
import io.micrometer.core.instrument.simple.SimpleMeterRegistry;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

/**
 * HI-01: Test that MicrometerBridge correctly registers all metrics.
 */
class MicrometerBridgeTest {

    @Test
    void testBindRegistersAllMetrics() {
        SimpleMeterRegistry registry = new SimpleMeterRegistry();
        MicrometerBridge.bind(registry);

        List<Meter> meters = registry.getMeters();
        assertFalse(meters.isEmpty(), "Metrics should be registered");

        // Verify key metrics exist
        assertNotNull(registry.find("expressgateway.connections.active").gauge(),
                "Active connections gauge should be registered");
        assertNotNull(registry.find("expressgateway.bandwidth.tx.bytes").functionCounter(),
                "Bandwidth TX counter should be registered");
        assertNotNull(registry.find("expressgateway.bandwidth.rx.bytes").functionCounter(),
                "Bandwidth RX counter should be registered");
        assertNotNull(registry.find("expressgateway.latency.p50").gauge(),
                "Latency p50 gauge should be registered");
        assertNotNull(registry.find("expressgateway.latency.p99").gauge(),
                "Latency p99 gauge should be registered");
        assertNotNull(registry.find("expressgateway.errors.connection").functionCounter(),
                "Connection errors counter should be registered");
        assertNotNull(registry.find("expressgateway.errors.tls").functionCounter(),
                "TLS errors counter should be registered");
    }

    @Test
    void testMetricValuesReflectRecorder() {
        SimpleMeterRegistry registry = new SimpleMeterRegistry();
        MicrometerBridge.bind(registry);
        StandardEdgeNetworkMetricRecorder recorder = StandardEdgeNetworkMetricRecorder.INSTANCE;

        // Record some metrics
        recorder.incrementActiveConnections();
        recorder.incrementActiveConnections();

        // Verify the gauge reads live values
        double activeConns = registry.find("expressgateway.connections.active").gauge().value();
        assertTrue(activeConns >= 2.0, "Active connections should reflect recorded value");

        // Clean up
        recorder.decrementActiveConnections();
        recorder.decrementActiveConnections();
    }
}
