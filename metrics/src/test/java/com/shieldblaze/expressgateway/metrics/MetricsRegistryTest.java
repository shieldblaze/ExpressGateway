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

import org.junit.jupiter.api.Test;

import java.util.concurrent.atomic.LongAdder;

import static org.junit.jupiter.api.Assertions.*;

class MetricsRegistryTest {

    @Test
    void counterRegistrationAndRetrieval() {
        MetricsRegistry registry = new MetricsRegistry();

        LongAdder counter1 = registry.counter("requests.total");
        counter1.increment();
        counter1.increment();

        LongAdder counter2 = registry.counter("requests.total");
        assertSame(counter1, counter2, "Same name should return same counter");
        assertEquals(2, counter2.sum());
    }

    @Test
    void gaugeRegistration() {
        MetricsRegistry registry = new MetricsRegistry();

        registry.gauge("connections.active", () -> 42);

        assertTrue(registry.names().contains("connections.active"));
    }

    @Test
    void histogramRegistration() {
        MetricsRegistry registry = new MetricsRegistry();

        LatencyHistogram h = registry.histogram("request.latency");
        h.record(10);
        h.record(20);

        LatencyHistogram h2 = registry.histogram("request.latency");
        assertSame(h, h2);
        assertEquals(2, h2.count());
    }

    @Test
    void throughputMeterRegistration() {
        MetricsRegistry registry = new MetricsRegistry();

        ThroughputMeter m = registry.throughputMeter("network.throughput");
        m.markBytes(1024);

        ThroughputMeter m2 = registry.throughputMeter("network.throughput");
        assertSame(m, m2);
        assertEquals(1024, m2.totalBytes());
    }

    @Test
    void removeMetric() {
        MetricsRegistry registry = new MetricsRegistry();

        registry.counter("temp.counter");
        assertTrue(registry.names().contains("temp.counter"));

        assertTrue(registry.remove("temp.counter"));
        assertFalse(registry.names().contains("temp.counter"));

        assertFalse(registry.remove("nonexistent"));
    }

    @Test
    void allNames() {
        MetricsRegistry registry = new MetricsRegistry();

        registry.counter("c1");
        registry.gauge("g1", () -> 0);
        registry.histogram("h1");
        registry.throughputMeter("t1");

        var names = registry.names();
        assertTrue(names.contains("c1"));
        assertTrue(names.contains("g1"));
        assertTrue(names.contains("h1"));
        assertTrue(names.contains("t1"));
    }

    @Test
    void prometheusExport() {
        MetricsRegistry registry = new MetricsRegistry();

        registry.counter("http.requests").add(100);
        registry.gauge("connections.active", () -> 42);

        String prom = registry.exportPrometheus();
        assertNotNull(prom);
        assertTrue(prom.contains("http_requests 100"), "Should contain counter");
        assertTrue(prom.contains("connections_active 42"), "Should contain gauge");
        assertTrue(prom.contains("# TYPE"), "Should contain type hints");
    }

    @Test
    void jsonExport() {
        MetricsRegistry registry = new MetricsRegistry();

        registry.counter("http.requests").add(50);
        registry.gauge("connections.active", () -> 10);

        String json = registry.exportJson();
        assertNotNull(json);
        assertTrue(json.contains("http.requests"), "JSON should contain counter name");
        assertTrue(json.contains("50"), "JSON should contain counter value");
        assertTrue(json.contains("connections.active"), "JSON should contain gauge name");
    }

    @Test
    void prometheusExportWithHistogram() {
        MetricsRegistry registry = new MetricsRegistry();

        LatencyHistogram h = registry.histogram("request.latency");
        for (int i = 0; i < 100; i++) {
            h.record(i);
        }

        String prom = registry.exportPrometheus();
        assertTrue(prom.contains("request_latency"));
        assertTrue(prom.contains("quantile=\"0.5\""));
        assertTrue(prom.contains("quantile=\"0.99\""));
        assertTrue(prom.contains("_count"));
        assertTrue(prom.contains("_sum"));
    }

    @Test
    void prometheusExportWithThroughputMeter() {
        MetricsRegistry registry = new MetricsRegistry();

        ThroughputMeter m = registry.throughputMeter("network.throughput");
        m.mark(10, 5000);

        String prom = registry.exportPrometheus();
        assertTrue(prom.contains("network_throughput_total 10"));
        assertTrue(prom.contains("network_throughput_rate"));
    }
}
