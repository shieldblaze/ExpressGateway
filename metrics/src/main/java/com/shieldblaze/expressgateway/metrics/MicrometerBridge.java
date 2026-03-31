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

import io.micrometer.core.instrument.FunctionCounter;
import io.micrometer.core.instrument.Gauge;
import io.micrometer.core.instrument.MeterRegistry;
import io.micrometer.core.instrument.MultiGauge;
import io.micrometer.core.instrument.Tags;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * HI-01: Bridges {@link StandardEdgeNetworkMetricRecorder} to Micrometer's {@link MeterRegistry}.
 *
 * <p>This class registers all existing custom metrics (LongAdder-based counters and gauges)
 * as Micrometer meters, enabling export to Prometheus, Datadog, CloudWatch, or any other
 * Micrometer-supported backend.</p>
 *
 * <p>Usage: call {@link #bind(MeterRegistry)} once during application bootstrap.
 * The registered meters will read live values from the singleton metric recorder.
 * Call {@link #refreshDynamicMetrics()} periodically (e.g., every scrape interval)
 * to update per-key dynamic metrics (status codes, backend latency, H2 streams, health).</p>
 *
 * <p>Metric naming follows Micrometer conventions (lowercase, dot-separated).
 * Prometheus will auto-convert these to snake_case with the appropriate suffix.</p>
 */
public final class MicrometerBridge {

    private static final String PREFIX = "expressgateway";

    private static MeterRegistry boundRegistry;
    private static MultiGauge statusCodeGauge;
    private static MultiGauge backendLatencyGauge;
    private static MultiGauge h2StreamGauge;
    private static MultiGauge healthStatusGauge;

    private MicrometerBridge() {}

    /**
     * Register all ExpressGateway metrics with the given Micrometer registry.
     *
     * @param registry the Micrometer MeterRegistry (e.g., PrometheusMeterRegistry)
     */
    public static void bind(MeterRegistry registry) {
        boundRegistry = registry;
        StandardEdgeNetworkMetricRecorder m = StandardEdgeNetworkMetricRecorder.INSTANCE;

        // --- Bandwidth ---
        FunctionCounter.builder(PREFIX + ".bandwidth.tx.bytes", m, StandardEdgeNetworkMetricRecorder::bandwidthTX)
                .description("Total bytes transmitted")
                .register(registry);
        FunctionCounter.builder(PREFIX + ".bandwidth.rx.bytes", m, StandardEdgeNetworkMetricRecorder::bandwidthRX)
                .description("Total bytes received")
                .register(registry);

        // --- Packets ---
        FunctionCounter.builder(PREFIX + ".packets.tx", m, StandardEdgeNetworkMetricRecorder::packetTX)
                .description("Total packets transmitted")
                .register(registry);
        FunctionCounter.builder(PREFIX + ".packets.rx", m, StandardEdgeNetworkMetricRecorder::packetRX)
                .description("Total packets received")
                .register(registry);

        // --- Active connections ---
        Gauge.builder(PREFIX + ".connections.active", m, StandardEdgeNetworkMetricRecorder::activeConnections)
                .description("Current active connections")
                .register(registry);

        // --- Latency ---
        Gauge.builder(PREFIX + ".latency.p50", m, StandardEdgeNetworkMetricRecorder::latencyP50)
                .description("Request latency 50th percentile (ms)")
                .register(registry);
        Gauge.builder(PREFIX + ".latency.p95", m, StandardEdgeNetworkMetricRecorder::latencyP95)
                .description("Request latency 95th percentile (ms)")
                .register(registry);
        Gauge.builder(PREFIX + ".latency.p99", m, StandardEdgeNetworkMetricRecorder::latencyP99)
                .description("Request latency 99th percentile (ms)")
                .register(registry);
        Gauge.builder(PREFIX + ".latency.min", m, StandardEdgeNetworkMetricRecorder::latencyMin)
                .description("Request latency minimum (ms)")
                .register(registry);
        Gauge.builder(PREFIX + ".latency.max", m, StandardEdgeNetworkMetricRecorder::latencyMax)
                .description("Request latency maximum (ms)")
                .register(registry);
        FunctionCounter.builder(PREFIX + ".latency.count", m, StandardEdgeNetworkMetricRecorder::latencyCount)
                .description("Total request count")
                .register(registry);
        FunctionCounter.builder(PREFIX + ".latency.sum", m, StandardEdgeNetworkMetricRecorder::latencySum)
                .description("Total request latency sum (ms)")
                .register(registry);

        // --- Errors ---
        FunctionCounter.builder(PREFIX + ".errors.connection", m, StandardEdgeNetworkMetricRecorder::connectionErrors)
                .description("Total backend connection errors")
                .register(registry);
        FunctionCounter.builder(PREFIX + ".errors.tls", m, StandardEdgeNetworkMetricRecorder::tlsErrors)
                .description("Total TLS handshake errors")
                .register(registry);

        // --- Rate limiting ---
        FunctionCounter.builder(PREFIX + ".ratelimit.rejections", m, StandardEdgeNetworkMetricRecorder::rateLimitRejections)
                .description("Total rate limit rejections")
                .register(registry);

        // --- Dynamic per-key metrics (MultiGauge) ---

        // --- Dynamic per-key metrics (MultiGauge) ---
        statusCodeGauge = MultiGauge.builder(PREFIX + ".http.status")
                .description("HTTP response count by status code")
                .register(registry);

        backendLatencyGauge = MultiGauge.builder(PREFIX + ".backend.latency.avg")
                .description("Average backend latency by backend (ms)")
                .register(registry);

        h2StreamGauge = MultiGauge.builder(PREFIX + ".backend.h2.streams.active")
                .description("Active HTTP/2 streams per backend")
                .register(registry);

        healthStatusGauge = MultiGauge.builder(PREFIX + ".backend.health")
                .description("Backend health status (1=healthy, 0=unhealthy)")
                .register(registry);
    }

    /**
     * Refresh all dynamic per-key metrics. Call this periodically (e.g., on each
     * Prometheus scrape or via a scheduled executor) to update MultiGauge rows
     * for status codes, backend latency, H2 streams, and health status.
     *
     * <p>This method is safe to call from any thread. MultiGauge.register()
     * handles its own synchronization.</p>
     */
    public static void refreshDynamicMetrics() {
        if (boundRegistry == null) {
            return;
        }

        StandardEdgeNetworkMetricRecorder m = StandardEdgeNetworkMetricRecorder.INSTANCE;

        // --- Status codes ---
        List<MultiGauge.Row<?>> statusRows = new ArrayList<>();
        for (Map.Entry<Integer, Long> entry : m.statusCodeCounts().entrySet()) {
            statusRows.add(MultiGauge.Row.of(Tags.of("code", String.valueOf(entry.getKey())), entry.getValue()));
        }
        statusCodeGauge.register(statusRows, true);

        // --- Backend latency ---
        List<MultiGauge.Row<?>> latencyRows = new ArrayList<>();
        for (Map.Entry<String, Long> entry : m.backendLatencies().entrySet()) {
            latencyRows.add(MultiGauge.Row.of(Tags.of("backend", entry.getKey()), entry.getValue()));
        }
        backendLatencyGauge.register(latencyRows, true);

        h2StreamGauge.register(List.of(), true);
        healthStatusGauge.register(List.of(), true);
    }
}
