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

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.util.Collections;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.LongAdder;
import java.util.function.LongSupplier;
import java.util.stream.Collectors;

/**
 * Central registry for all ExpressGateway metrics.
 *
 * <p>Provides metric lifecycle management, named counter/gauge registration,
 * and export in Prometheus and JSON formats.</p>
 *
 * <p>Thread safety: All internal structures use concurrent collections.
 * Virtual-thread safe (no synchronized blocks, no thread-local dependencies).</p>
 */
public final class MetricsRegistry {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    private final ConcurrentHashMap<String, LongAdder> counters = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LongSupplier> gauges = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LatencyHistogram> histograms = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, ThroughputMeter> throughputMeters = new ConcurrentHashMap<>();

    /**
     * Register or retrieve a named counter.
     *
     * @param name the metric name
     * @return the counter (created if not already present)
     */
    public LongAdder counter(String name) {
        return counters.computeIfAbsent(name, k -> new LongAdder());
    }

    /**
     * Register a gauge (a function that returns the current value).
     *
     * @param name     the metric name
     * @param supplier function returning the gauge value
     */
    public void gauge(String name, LongSupplier supplier) {
        gauges.put(name, supplier);
    }

    /**
     * Register or retrieve a named latency histogram.
     *
     * @param name the metric name
     * @return the histogram (created if not already present)
     */
    public LatencyHistogram histogram(String name) {
        return histograms.computeIfAbsent(name, k -> new LatencyHistogram());
    }

    /**
     * Register or retrieve a named throughput meter.
     *
     * @param name the metric name
     * @return the throughput meter (created if not already present)
     */
    public ThroughputMeter throughputMeter(String name) {
        return throughputMeters.computeIfAbsent(name, k -> new ThroughputMeter());
    }

    /**
     * Remove a metric by name.
     *
     * @param name the metric name
     * @return true if a metric was removed
     */
    public boolean remove(String name) {
        boolean removed = false;
        removed |= counters.remove(name) != null;
        removed |= gauges.remove(name) != null;
        removed |= histograms.remove(name) != null;
        removed |= throughputMeters.remove(name) != null;
        return removed;
    }

    /**
     * Returns all registered metric names.
     */
    public Set<String> names() {
        Set<String> names = ConcurrentHashMap.newKeySet();
        names.addAll(counters.keySet());
        names.addAll(gauges.keySet());
        names.addAll(histograms.keySet());
        names.addAll(throughputMeters.keySet());
        return Collections.unmodifiableSet(names);
    }

    /**
     * Export all metrics in Prometheus text exposition format.
     */
    public String exportPrometheus() {
        StringBuilder sb = new StringBuilder(4096);

        for (var entry : counters.entrySet()) {
            String name = prometheusName(entry.getKey());
            sb.append("# TYPE ").append(name).append(" counter\n");
            sb.append(name).append(' ').append(entry.getValue().sum()).append('\n');
        }

        for (var entry : gauges.entrySet()) {
            String name = prometheusName(entry.getKey());
            sb.append("# TYPE ").append(name).append(" gauge\n");
            sb.append(name).append(' ').append(entry.getValue().getAsLong()).append('\n');
        }

        for (var entry : histograms.entrySet()) {
            String name = prometheusName(entry.getKey());
            LatencyHistogram h = entry.getValue();
            sb.append("# TYPE ").append(name).append(" summary\n");
            sb.append(name).append("{quantile=\"0.5\"} ").append(h.p50()).append('\n');
            sb.append(name).append("{quantile=\"0.95\"} ").append(h.p95()).append('\n');
            sb.append(name).append("{quantile=\"0.99\"} ").append(h.p99()).append('\n');
            sb.append(name).append("{quantile=\"0.999\"} ").append(h.p999()).append('\n');
            sb.append(name).append("_count ").append(h.count()).append('\n');
            sb.append(name).append("_sum ").append(h.sum()).append('\n');
        }

        for (var entry : throughputMeters.entrySet()) {
            String name = prometheusName(entry.getKey());
            ThroughputMeter m = entry.getValue();
            sb.append("# TYPE ").append(name).append("_total counter\n");
            sb.append(name).append("_total ").append(m.totalCount()).append('\n');
            sb.append("# TYPE ").append(name).append("_rate gauge\n");
            sb.append(name).append("_rate ").append(String.format("%.2f", m.rate())).append('\n');
        }

        return sb.toString();
    }

    /**
     * Export all metrics as JSON.
     */
    public String exportJson() {
        Map<String, Object> snapshot = new ConcurrentHashMap<>();

        Map<String, Long> counterSnapshot = counters.entrySet().stream()
                .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().sum()));
        if (!counterSnapshot.isEmpty()) snapshot.put("counters", counterSnapshot);

        Map<String, Long> gaugeSnapshot = gauges.entrySet().stream()
                .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().getAsLong()));
        if (!gaugeSnapshot.isEmpty()) snapshot.put("gauges", gaugeSnapshot);

        Map<String, Map<String, Long>> histSnapshot = histograms.entrySet().stream()
                .collect(Collectors.toMap(Map.Entry::getKey, e -> {
                    LatencyHistogram h = e.getValue();
                    Map<String, Long> m = new ConcurrentHashMap<>();
                    m.put("count", h.count());
                    m.put("sum", h.sum());
                    m.put("min", h.min());
                    m.put("max", h.max());
                    m.put("p50", h.p50());
                    m.put("p95", h.p95());
                    m.put("p99", h.p99());
                    m.put("p999", h.p999());
                    return m;
                }));
        if (!histSnapshot.isEmpty()) snapshot.put("histograms", histSnapshot);

        Map<String, Map<String, Object>> meterSnapshot = throughputMeters.entrySet().stream()
                .collect(Collectors.toMap(Map.Entry::getKey, e -> {
                    ThroughputMeter m = e.getValue();
                    Map<String, Object> data = new ConcurrentHashMap<>();
                    data.put("total", m.totalCount());
                    data.put("rate", m.rate());
                    return data;
                }));
        if (!meterSnapshot.isEmpty()) snapshot.put("throughputMeters", meterSnapshot);

        return GSON.toJson(snapshot);
    }

    private static String prometheusName(String name) {
        return name.replace('.', '_').replace('-', '_');
    }
}
