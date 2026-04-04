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
import io.netty.buffer.ByteBufHolder;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

import java.lang.invoke.VarHandle;
import java.util.Arrays;
import java.util.Collections;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.LongAdder;
import java.util.stream.Collectors;

/**
 * Production-grade edge network metric recorder.
 *
 * <p>All counters are monotonic (never reset in normal operation).
 * Consumers should snapshot and diff to compute rates.</p>
 *
 * <p>Thread safety: All fields use lock-free atomic structures.
 * {@link LongAdder} is used for high-contention write paths (every channelRead/write call)
 * because LongAdder's striped cell design reduces CAS contention across event loop threads.</p>
 *
 * <p>The latency histogram uses a fixed-size circular buffer with atomic write index
 * for lock-free append; percentile computation takes a snapshot for consistency.</p>
 */
@ChannelHandler.Sharable
public final class StandardEdgeNetworkMetricRecorder extends ChannelDuplexHandler implements EdgeNetworkMetric {

    public static final StandardEdgeNetworkMetricRecorder INSTANCE = new StandardEdgeNetworkMetricRecorder();

    // --- Bandwidth/packet counters (monotonic, LongAdder for low contention) ---
    private final LongAdder bandwidthTXCounter = new LongAdder();
    private final LongAdder bandwidthRXCounter = new LongAdder();
    private final LongAdder packetTXCounter = new LongAdder();
    private final LongAdder packetRXCounter = new LongAdder();

    // --- Active connection gauge ---
    private final AtomicLong activeConnectionCount = new AtomicLong();

    // --- HTTP status code counters ---
    private final ConcurrentHashMap<Integer, LongAdder> statusCodes = new ConcurrentHashMap<>();

    // --- Latency histogram (circular buffer) ---
    private static final int LATENCY_BUFFER_SIZE = 1024;
    private static final int LATENCY_BUFFER_MASK = LATENCY_BUFFER_SIZE - 1;
    private final long[] latencyBuffer = new long[LATENCY_BUFFER_SIZE];
    private final AtomicLong latencyWriteIndex = new AtomicLong();
    private final LongAdder latencyTotalCount = new LongAdder();
    private final LongAdder latencyTotalSum = new LongAdder();
    private final AtomicLong latencyMinValue = new AtomicLong(Long.MAX_VALUE);
    private final AtomicLong latencyMaxValue = new AtomicLong(Long.MIN_VALUE);

    // --- Error counters ---
    private final LongAdder connectionErrorCount = new LongAdder();
    private final LongAdder tlsErrorCount = new LongAdder();
    private final LongAdder rateLimitRejectionCount = new LongAdder();

    // --- Per-backend latency ---
    private final ConcurrentHashMap<String, LongAdder> backendLatencySum = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LongAdder> backendLatencyCount = new ConcurrentHashMap<>();

    // --- Per-protocol metrics ---
    private final ConcurrentHashMap<String, LongAdder> protocolByteCounters = new ConcurrentHashMap<>();
    private final ConcurrentHashMap<String, LongAdder> protocolRequestCounters = new ConcurrentHashMap<>();

    // --- Connection pool counters ---
    private final LongAdder poolHitCount = new LongAdder();
    private final LongAdder poolMissCount = new LongAdder();

    // --- H2 stream and retry counters ---
    private final ConcurrentHashMap<String, AtomicInteger> activeH2StreamCounts =
            new ConcurrentHashMap<>();
    private final LongAdder retryAttemptCount = new LongAdder();

    // --- Per-backend connection count ---
    private final ConcurrentHashMap<String, AtomicInteger> backendConnectionCounts =
            new ConcurrentHashMap<>();

    /**
     * Maximum number of distinct keys allowed in per-key maps to prevent
     * unbounded memory growth from cardinality explosion (e.g., unique backend
     * names generated from attacker-controlled input).
     */
    private static final int MAX_PER_KEY_MAP_SIZE = 10_000;

    private StandardEdgeNetworkMetricRecorder() {
        // Singleton
    }

    // ========================
    // Netty ChannelHandler
    // ========================

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        packetRXCounter.increment();

        if (msg instanceof ByteBuf byteBuf) {
            bandwidthRXCounter.add(byteBuf.readableBytes());
        } else if (msg instanceof ByteBufHolder holder) {
            bandwidthRXCounter.add(holder.content().readableBytes());
        }

        ctx.fireChannelRead(msg);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        packetTXCounter.increment();

        if (msg instanceof ByteBuf byteBuf) {
            bandwidthTXCounter.add(byteBuf.readableBytes());
        } else if (msg instanceof ByteBufHolder holder) {
            bandwidthTXCounter.add(holder.content().readableBytes());
        }

        ctx.write(msg, promise);
    }

    // ========================
    // Bandwidth / Packet (monotonic)
    // ========================

    @Override
    public long bandwidthTX() {
        return bandwidthTXCounter.sum();
    }

    @Override
    public long bandwidthRX() {
        return bandwidthRXCounter.sum();
    }

    @Override
    public long packetTX() {
        return packetTXCounter.sum();
    }

    @Override
    public long packetRX() {
        return packetRXCounter.sum();
    }

    // ========================
    // Active connections
    // ========================

    @Override
    public void incrementActiveConnections() {
        activeConnectionCount.incrementAndGet();
    }

    @Override
    public void decrementActiveConnections() {
        // Floor check to prevent the gauge from going negative due to
        // mismatched increment/decrement calls.
        long prev;
        do {
            prev = activeConnectionCount.get();
            if (prev <= 0) {
                return;
            }
        } while (!activeConnectionCount.compareAndSet(prev, prev - 1));
    }

    @Override
    public long activeConnections() {
        return activeConnectionCount.get();
    }

    // ========================
    // HTTP status codes
    // ========================

    @Override
    public void recordStatusCode(int statusCode) {
        LongAdder adder = statusCodes.get(statusCode);
        if (adder != null) {
            adder.increment();
        } else if (statusCodes.size() < MAX_PER_KEY_MAP_SIZE) {
            statusCodes.computeIfAbsent(statusCode, k -> new LongAdder()).increment();
        }
    }

    @Override
    public Map<Integer, Long> statusCodeCounts() {
        return Collections.unmodifiableMap(
                statusCodes.entrySet().stream()
                        .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().sum()))
        );
    }

    // ========================
    // Latency histogram
    // ========================

    @Override
    public void recordLatency(long latencyMs) {
        long idx = latencyWriteIndex.getAndIncrement();
        latencyBuffer[(int) (idx & LATENCY_BUFFER_MASK)] = latencyMs;
        // Ensure the latencyBuffer write is visible before incrementing
        // the count, so readers computing percentiles never see stale
        // (zero or previous) values in slots that the count says are filled.
        VarHandle.storeStoreFence();
        latencyTotalCount.increment();
        latencyTotalSum.add(latencyMs);

        // Update min atomically
        long currentMin;
        do {
            currentMin = latencyMinValue.get();
            if (latencyMs >= currentMin) break;
        } while (!latencyMinValue.compareAndSet(currentMin, latencyMs));

        // Update max atomically
        long currentMax;
        do {
            currentMax = latencyMaxValue.get();
            if (latencyMs <= currentMax) break;
        } while (!latencyMaxValue.compareAndSet(currentMax, latencyMs));
    }

    @Override
    public long latencyCount() {
        return latencyTotalCount.sum();
    }

    @Override
    public long latencySum() {
        return latencyTotalSum.sum();
    }

    @Override
    public long latencyMin() {
        long val = latencyMinValue.get();
        return val == Long.MAX_VALUE ? 0 : val;
    }

    @Override
    public long latencyMax() {
        long val = latencyMaxValue.get();
        return val == Long.MIN_VALUE ? 0 : val;
    }

    @Override
    public long latencyP50() {
        return computePercentile(50);
    }

    @Override
    public long latencyP95() {
        return computePercentile(95);
    }

    @Override
    public long latencyP99() {
        return computePercentile(99);
    }

    private long computePercentile(int percentile) {
        long totalSamples = latencyTotalCount.sum();
        if (totalSamples == 0) {
            return 0;
        }

        int sampleCount = (int) Math.min(totalSamples, LATENCY_BUFFER_SIZE);
        long[] snapshot = new long[sampleCount];
        long currentIdx = latencyWriteIndex.get();
        for (int i = 0; i < sampleCount; i++) {
            snapshot[i] = latencyBuffer[(int) ((currentIdx - sampleCount + i) & LATENCY_BUFFER_MASK)];
        }

        Arrays.sort(snapshot);
        int rank = (int) Math.ceil(percentile / 100.0 * sampleCount) - 1;
        return snapshot[Math.max(0, Math.min(rank, sampleCount - 1))];
    }

    // ========================
    // Error counters
    // ========================

    @Override
    public void recordConnectionError() {
        connectionErrorCount.increment();
    }

    @Override
    public long connectionErrors() {
        return connectionErrorCount.sum();
    }

    @Override
    public void recordTlsError() {
        tlsErrorCount.increment();
    }

    @Override
    public long tlsErrors() {
        return tlsErrorCount.sum();
    }

    @Override
    public void recordRateLimitRejection() {
        rateLimitRejectionCount.increment();
    }

    @Override
    public long rateLimitRejections() {
        return rateLimitRejectionCount.sum();
    }

    // ========================
    // Per-backend latency
    // ========================

    @Override
    public void recordBackendLatency(String backend, long latencyMs) {
        LongAdder sum = backendLatencySum.get(backend);
        LongAdder count = backendLatencyCount.get(backend);
        if (sum != null && count != null) {
            sum.add(latencyMs);
            count.increment();
        } else if (backendLatencySum.size() < MAX_PER_KEY_MAP_SIZE) {
            backendLatencySum.computeIfAbsent(backend, k -> new LongAdder()).add(latencyMs);
            backendLatencyCount.computeIfAbsent(backend, k -> new LongAdder()).increment();
        }
    }

    @Override
    public Map<String, Long> backendLatencies() {
        return Collections.unmodifiableMap(
                backendLatencyCount.entrySet().stream()
                        .collect(Collectors.toMap(
                                Map.Entry::getKey,
                                e -> {
                                    long count = e.getValue().sum();
                                    if (count == 0) return 0L;
                                    LongAdder sum = backendLatencySum.get(e.getKey());
                                    return sum != null ? sum.sum() / count : 0L;
                                }
                        ))
        );
    }

    // ========================
    // Per-protocol metrics
    // ========================

    @Override
    public void recordProtocolBytes(String protocol, long bytes) {
        LongAdder adder = protocolByteCounters.get(protocol);
        if (adder != null) {
            adder.add(bytes);
        } else if (protocolByteCounters.size() < MAX_PER_KEY_MAP_SIZE) {
            protocolByteCounters.computeIfAbsent(protocol, k -> new LongAdder()).add(bytes);
        }
    }

    @Override
    public Map<String, Long> protocolBytes() {
        return Collections.unmodifiableMap(
                protocolByteCounters.entrySet().stream()
                        .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().sum()))
        );
    }

    @Override
    public void recordProtocolRequest(String protocol) {
        LongAdder adder = protocolRequestCounters.get(protocol);
        if (adder != null) {
            adder.increment();
        } else if (protocolRequestCounters.size() < MAX_PER_KEY_MAP_SIZE) {
            protocolRequestCounters.computeIfAbsent(protocol, k -> new LongAdder()).increment();
        }
    }

    @Override
    public Map<String, Long> protocolRequests() {
        return Collections.unmodifiableMap(
                protocolRequestCounters.entrySet().stream()
                        .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().sum()))
        );
    }

    // ========================
    // Per-backend connections
    // ========================

    @Override
    public void recordBackendConnections(String backend, int count) {
        AtomicInteger ai = backendConnectionCounts.get(backend);
        if (ai != null) {
            ai.set(count);
        } else if (backendConnectionCounts.size() < MAX_PER_KEY_MAP_SIZE) {
            backendConnectionCounts.computeIfAbsent(backend,
                    k -> new AtomicInteger()).set(count);
        }
    }

    @Override
    public Map<String, Integer> backendConnections() {
        return Collections.unmodifiableMap(
                backendConnectionCounts.entrySet().stream()
                        .collect(Collectors.toMap(Map.Entry::getKey, e -> e.getValue().get()))
        );
    }

    // ========================
    // Connection pool metrics
    // ========================

    @Override
    public void recordPoolHit() {
        poolHitCount.increment();
    }

    @Override
    public void recordPoolMiss() {
        poolMissCount.increment();
    }

    @Override
    public long poolHits() {
        return poolHitCount.sum();
    }

    @Override
    public long poolMisses() {
        return poolMissCount.sum();
    }

    // ========================
    // H2 stream and retry metrics
    // ========================

    @Override
    public void recordActiveH2Streams(String backend, int count) {
        AtomicInteger ai = activeH2StreamCounts.get(backend);
        if (ai != null) {
            ai.set(count);
        } else if (activeH2StreamCounts.size() < MAX_PER_KEY_MAP_SIZE) {
            activeH2StreamCounts.computeIfAbsent(backend,
                    k -> new AtomicInteger()).set(count);
        }
    }

    @Override
    public void recordRetryAttempt() {
        retryAttemptCount.increment();
    }

    @Override
    public long retryAttempts() {
        return retryAttemptCount.sum();
    }

    // ========================
    // Deprecated reset
    // ========================

    @Override
    @Deprecated(forRemoval = true)
    public void reset() {
        // No-op: monotonic counters should not be reset.
    }
}
