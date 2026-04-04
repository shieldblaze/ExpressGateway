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
package com.shieldblaze.expressgateway.configuration.http;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;
import com.shieldblaze.expressgateway.configuration.Configuration;
import lombok.Getter;
import lombok.Setter;
import lombok.ToString;
import lombok.experimental.Accessors;

import java.util.Set;

import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_CHUNK_SIZE;
import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_HEADER_SIZE;
import static io.netty.handler.codec.http.HttpObjectDecoder.DEFAULT_MAX_INITIAL_LINE_LENGTH;

/**
 * Configuration for HTTP/1.1 and HTTP/2 protocol settings.
 */
@Getter
@Setter
@Accessors(fluent = true, chain = true)
@ToString
public final class HttpConfiguration implements Configuration<HttpConfiguration> {

    @JsonProperty
    private int maxInitialLineLength;

    @JsonProperty
    private int maxHeaderSize;

    @JsonProperty
    private int maxChunkSize;

    @JsonProperty
    private int compressionThreshold;

    @JsonProperty
    private int deflateCompressionLevel;

    @JsonProperty
    private int brotliCompressionLevel;

    /**
     * Maximum number of concurrent HTTP/2 streams per connection.
     * Per RFC 9113 Section 6.5.2 (SETTINGS_MAX_CONCURRENT_STREAMS).
     */
    @JsonProperty
    private long maxConcurrentStreams;

    /**
     * Backend response timeout in seconds. If no data is received from
     * the backend within this period, the connection is closed.
     */
    @JsonProperty
    private long backendResponseTimeoutSeconds;

    /**
     * Maximum request body size in bytes. Requests exceeding this limit
     * receive a 413 response. Analogous to nginx's client_max_body_size.
     */
    @JsonProperty
    private long maxRequestBodySize;

    /**
     * DEF-H2-02: Maximum response body size in bytes. Responses exceeding this limit
     * trigger a 502 Bad Gateway error. Prevents a rogue backend from exhausting proxy
     * memory. A value of 0 disables the limit (default). Analogous to nginx's
     * proxy_max_temp_file_size.
     */
    @JsonProperty
    private long maxResponseBodySize;

    /**
     * HTTP/2 SETTINGS_INITIAL_WINDOW_SIZE (RFC 9113 Section 6.5.2).
     * Controls the initial flow-control window size for new streams.
     * Larger values reduce round-trips for high-bandwidth transfers;
     * smaller values limit memory consumption per stream.
     */
    @JsonProperty
    private int initialWindowSize;

    /**
     * F-13: Maximum aggregate request body bytes across all streams on a single
     * HTTP/2 connection. Prevents a single connection from consuming unbounded
     * memory via many concurrent streams. Default 256 MB.
     */
    @JsonProperty
    private long maxConnectionBodySize;

    /**
     * HTTP/2 connection-level flow control window size in bytes (RFC 9113 Section 6.9.2).
     * The default per the RFC is 65535, which bottlenecks throughput under heavy multiplexing.
     * This value controls the initial connection-level window by sending a WINDOW_UPDATE on
     * stream 0 after the connection preface. Legal range: 65535 to 2^31-1 (2147483647).
     *
     * <p>Back-of-envelope: at 100 concurrent streams, the default 64 KB connection window
     * limits aggregate throughput to ~64 KB per RTT across ALL streams. With a 1 MB connection
     * window, each RTT can deliver ~1 MB aggregate, dramatically improving utilization on
     * high-bandwidth-delay-product links.</p>
     */
    @JsonProperty
    private int h2ConnectionWindowSize;

    /**
     * F-14: HTTP/2 SETTINGS_MAX_HEADER_LIST_SIZE (RFC 9113 Section 6.5.2).
     * Controls the maximum size of header list that the HPACK decoder will accept.
     * Prevents memory exhaustion from oversized header blocks. Default is Netty's
     * {@code DEFAULT_HEADER_LIST_SIZE} (8192 bytes).
     */
    @JsonProperty
    private long maxHeaderListSize;

    /**
     * SEC-02: Maximum time in seconds to wait for the first complete set of
     * request headers on a new connection. Defends against Slowloris attacks
     * where an attacker sends headers very slowly to hold connections open.
     * Default is 30 seconds.
     */
    @JsonProperty
    private long requestHeaderTimeoutSeconds;

    /**
     * ME-04: Maximum time in seconds for the complete request body to be received
     * after headers arrive. Defends against slow-POST attacks where a client sends
     * body data at 1 byte/second, bypassing the idle timeout (which resets on each
     * read). The timer starts when an HttpRequest with a body is received and is
     * cancelled when LastHttpContent arrives. Default is 60 seconds.
     * Set to 0 to disable.
     */
    @JsonProperty
    private long requestBodyTimeoutSeconds;

    /**
     * Maximum number of idle HTTP/1.1 backend connections to keep per backend Node.
     * Analogous to Nginx's {@code keepalive} directive per upstream block.
     * Connections beyond this limit are closed instead of returned to the pool.
     */
    @JsonProperty
    private int maxH1ConnectionsPerNode;

    /**
     * Maximum number of HTTP/2 backend connections to maintain per backend Node.
     * When all connections reach {@code maxConcurrentStreams}, a new connection is
     * created up to this limit. Analogous to Envoy's cluster connection pool sizing.
     */
    @JsonProperty
    private int maxH2ConnectionsPerNode;

    /**
     * RES-02: Maximum time in seconds that an HTTP/1.1 backend connection may sit
     * idle in the pool before being proactively evicted. Prevents stale connections
     * that the backend has already closed (via TCP keepalive or server timeout)
     * from being returned to callers and failing on first use.
     *
     * <p>Analogous to Nginx's {@code keepalive_timeout} for upstream connections
     * and HAProxy's {@code timeout server} idle behavior. Set to 0 to disable
     * idle eviction (not recommended for production).</p>
     */
    @JsonProperty
    private long poolIdleTimeoutSeconds;

    /**
     * RES-DRAIN: Graceful shutdown drain timeout in milliseconds for HTTP/2 connections.
     * When closing a frontend HTTP/2 connection, a GOAWAY frame is sent first and the
     * actual channel close is delayed by this duration to allow in-flight streams to
     * complete. This prevents abruptly terminating active request/response exchanges.
     *
     * <p>Analogous to Nginx's {@code worker_shutdown_timeout} and HAProxy's
     * {@code timeout server fin}. RFC 9113 Section 6.8 recommends sending GOAWAY
     * and then waiting for peers to close their streams gracefully.</p>
     *
     * <p>Default is 5000ms (5 seconds). Set to 0 to disable drain and close immediately.</p>
     */
    @JsonProperty
    private long gracefulShutdownDrainMs;

    /**
     * Configurable method whitelist. Only methods in this set are forwarded to backends.
     * Requests with methods not in this set receive 405 Method Not Allowed.
     * When {@code null}, the default set is used: GET, HEAD, POST, PUT, DELETE, PATCH, OPTIONS.
     */
    @JsonProperty
    private Set<String> allowedMethods;

    @JsonProperty
    private RetryConfiguration retryConfiguration;

    public static final HttpConfiguration DEFAULT = new HttpConfiguration();

    static {
        DEFAULT.maxInitialLineLength = DEFAULT_MAX_INITIAL_LINE_LENGTH;
        DEFAULT.maxHeaderSize = DEFAULT_MAX_HEADER_SIZE;
        DEFAULT.maxChunkSize = DEFAULT_MAX_CHUNK_SIZE;
        DEFAULT.compressionThreshold = 1024;
        DEFAULT.deflateCompressionLevel = 6;
        DEFAULT.brotliCompressionLevel = 4;
        DEFAULT.maxConcurrentStreams = 100;
        DEFAULT.backendResponseTimeoutSeconds = 60;
        DEFAULT.maxRequestBodySize = 10L * 1024 * 1024; // 10 MB
        DEFAULT.maxResponseBodySize = 0; // 0 = unlimited (default — client requested the data)
        DEFAULT.initialWindowSize = 1048576; // 1 MB — balances throughput vs per-stream memory
        DEFAULT.h2ConnectionWindowSize = 1048576; // 1 MB — matches per-stream window; prevents connection-level bottleneck
        DEFAULT.maxConnectionBodySize = 256L * 1024 * 1024; // 256 MB
        DEFAULT.maxHeaderListSize = 8192; // Netty DEFAULT_HEADER_LIST_SIZE
        DEFAULT.requestHeaderTimeoutSeconds = 30;
        DEFAULT.requestBodyTimeoutSeconds = 60; // ME-04: 60 seconds max for complete request body delivery
        DEFAULT.maxH1ConnectionsPerNode = 32;
        DEFAULT.maxH2ConnectionsPerNode = 4;
        DEFAULT.poolIdleTimeoutSeconds = 60;
        DEFAULT.gracefulShutdownDrainMs = 5000; // 5 seconds — per RFC 9113 Section 6.8, allow in-flight streams to complete
        DEFAULT.allowedMethods = Set.of("GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS");
        DEFAULT.retryConfiguration = RetryConfiguration.DEFAULT;
    }

    HttpConfiguration() {
        // Prevent outside initialization
    }

    public Set<String> allowedMethods() {
        if (allowedMethods == null) {
            return Set.of("GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS");
        }
        return allowedMethods;
    }

    public RetryConfiguration retryConfiguration() {
        if (retryConfiguration == null) {
            return RetryConfiguration.DEFAULT;
        }
        return retryConfiguration;
    }

    /**
     * Validate all parameters of this configuration
     *
     * @return this class instance
     * @throws IllegalArgumentException If any value is invalid
     */
    @Override
    public HttpConfiguration validate() {
        NumberUtil.checkPositive(maxInitialLineLength, "MaxInitialLineLength");
        NumberUtil.checkPositive(maxHeaderSize, "MaxHeaderSize");
        NumberUtil.checkPositive(maxChunkSize, "MaxChunkSize");
        NumberUtil.checkZeroOrPositive(compressionThreshold, "CompressionThreshold");
        NumberUtil.checkInRange(deflateCompressionLevel, 0, 9, "DeflateCompressionLevel");
        NumberUtil.checkInRange(brotliCompressionLevel, 1, 11, "BrotliCompressionLevel");
        NumberUtil.checkPositive(maxConcurrentStreams, "MaxConcurrentStreams");
        NumberUtil.checkPositive(backendResponseTimeoutSeconds, "BackendResponseTimeoutSeconds");
        NumberUtil.checkPositive(maxRequestBodySize, "MaxRequestBodySize");
        NumberUtil.checkZeroOrPositive(maxResponseBodySize, "MaxResponseBodySize");
        // MED-28: RFC 9113 Section 6.9.2 — legal range is 0 to 2^31-1. Allow 0.
        NumberUtil.checkZeroOrPositive(initialWindowSize, "InitialWindowSize");
        // RFC 9113 Section 6.9.2 — connection-level window must be >= 65535 (the RFC default).
        // Values below 65535 would require shrinking the window, which is not supported.
        NumberUtil.checkPositive(h2ConnectionWindowSize, "H2ConnectionWindowSize");
        NumberUtil.checkPositive(maxConnectionBodySize, "MaxConnectionBodySize");
        NumberUtil.checkPositive(maxHeaderListSize, "MaxHeaderListSize");
        NumberUtil.checkPositive(requestHeaderTimeoutSeconds, "RequestHeaderTimeoutSeconds");
        NumberUtil.checkZeroOrPositive(requestBodyTimeoutSeconds, "RequestBodyTimeoutSeconds");
        NumberUtil.checkPositive(maxH1ConnectionsPerNode, "MaxH1ConnectionsPerNode");
        NumberUtil.checkPositive(maxH2ConnectionsPerNode, "MaxH2ConnectionsPerNode");
        NumberUtil.checkZeroOrPositive(poolIdleTimeoutSeconds, "PoolIdleTimeoutSeconds");
        NumberUtil.checkZeroOrPositive(gracefulShutdownDrainMs, "GracefulShutdownDrainMs");
        return this;
    }
}
