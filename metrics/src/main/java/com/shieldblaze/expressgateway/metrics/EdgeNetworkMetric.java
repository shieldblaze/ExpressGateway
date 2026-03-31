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

import java.util.Map;

/**
 * Metrics interface for edge network monitoring.
 *
 * <p>All counters are monotonically increasing. Consumers should compute rates
 * by diffing snapshots rather than resetting counters.</p>
 *
 * <p>Extends the original bandwidth/packet interface with connection gauges,
 * HTTP status tracking, latency histograms, error counters, per-backend
 * metrics, and per-protocol metrics.</p>
 */
public interface EdgeNetworkMetric {

    // --- Bandwidth and Packet counters ---

    long bandwidthTX();

    long bandwidthRX();

    long packetTX();

    long packetRX();

    // --- Active connection gauge ---

    void incrementActiveConnections();

    void decrementActiveConnections();

    long activeConnections();

    // --- HTTP status code counters ---

    void recordStatusCode(int statusCode);

    Map<Integer, Long> statusCodeCounts();

    // --- Request latency tracking ---

    void recordLatency(long latencyMs);

    long latencyCount();

    long latencySum();

    long latencyMin();

    long latencyMax();

    long latencyP50();

    long latencyP95();

    long latencyP99();

    // --- Backend connection errors ---

    void recordConnectionError();

    long connectionErrors();

    // --- TLS handshake errors ---

    void recordTlsError();

    long tlsErrors();

    // --- Rate limiter rejections ---

    void recordRateLimitRejection();

    long rateLimitRejections();

    // --- Per-backend latency ---

    void recordBackendLatency(String backend, long latencyMs);

    Map<String, Long> backendLatencies();

    // --- Per-protocol metrics ---

    /**
     * Record bytes for a specific protocol.
     *
     * @param protocol protocol identifier (e.g., "HTTP/1.1", "HTTP/2", "TCP", "UDP")
     * @param bytes    number of bytes
     */
    void recordProtocolBytes(String protocol, long bytes);

    /**
     * Get per-protocol byte counts.
     */
    Map<String, Long> protocolBytes();

    /**
     * Record a request for a specific protocol.
     */
    void recordProtocolRequest(String protocol);

    /**
     * Get per-protocol request counts.
     */
    Map<String, Long> protocolRequests();

    // --- Connection pool metrics ---

    void recordPoolHit();

    void recordPoolMiss();

    long poolHits();

    long poolMisses();

    // --- H2 stream and retry metrics ---

    void recordActiveH2Streams(String backend, int count);

    void recordRetryAttempt();

    long retryAttempts();

    // --- Per-backend connection count ---

    void recordBackendConnections(String backend, int count);

    Map<String, Integer> backendConnections();

    /**
     * @deprecated Metrics should be monotonic counters. Use snapshot-and-diff
     *             for rate computation. Retained for backward compatibility only.
     */
    @Deprecated(forRemoval = true)
    void reset();
}
