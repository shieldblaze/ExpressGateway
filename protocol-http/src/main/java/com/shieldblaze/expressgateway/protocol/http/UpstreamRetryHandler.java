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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.Channel;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http2.Http2Headers;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import java.net.ConnectException;
import java.net.InetSocketAddress;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.atomic.LongAdder;

/**
 * Handles upstream connection retry logic for failed backend connections.
 * <p>
 * Per RFC 9110 Section 9.2.2, idempotent methods (GET, HEAD, OPTIONS, PUT, DELETE, TRACE)
 * are safe to retry because repeating them produces the same result. POST and PATCH are
 * NOT idempotent and must not be retried to avoid duplicate side effects.
 * <p>
 * Retry is only attempted for <b>connection-level</b> failures (connection refused,
 * connection timeout) — never for response-level errors (e.g., 500, 503). This distinction
 * is critical: a response error means the backend accepted and processed the request,
 * so retrying could cause duplicate processing. A connection failure means the request
 * was never delivered to any backend.
 * <p>
 * This is consistent with how Nginx (proxy_next_upstream) and HAProxy (option redispatch)
 * handle upstream retries: they only retry when the backend was never reached.
 *
 * <h3>Thread Safety</h3>
 * Instances of this class are <b>not thread-safe</b> and must be used within a single
 * channel's event loop (which is the standard Netty threading model for inbound handlers).
 */
final class UpstreamRetryHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamRetryHandler.class);

    /**
     * Default maximum number of retry attempts before giving up and returning 502.
     */
    static final int DEFAULT_MAX_RETRIES = 2;

    /**
     * Global counter of total requests processed across all UpstreamRetryHandler instances.
     * Static because the retry budget must be enforced globally — a per-connection budget
     * would be ineffective against a retry storm where thousands of connections each
     * individually stay under budget but collectively overwhelm backends.
     *
     * <p>LongAdder is chosen over AtomicLong for lower contention under high concurrency:
     * multiple EventLoop threads increment their own cells, and the sum is only computed
     * on the budget-check path. The slight staleness of sum() is acceptable — the budget
     * is a probabilistic circuit breaker, not an exact accounting system.</p>
     */
    static final LongAdder totalRequests = new LongAdder();

    /**
     * Global counter of retry attempts across all UpstreamRetryHandler instances.
     * See {@link #totalRequests} for rationale on static + LongAdder.
     */
    static final LongAdder totalRetries = new LongAdder();

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final int maxRetries;

    /**
     * Maximum percentage of total requests that may be retries (0-100).
     * When {@code totalRetries.sum() > totalRequests.sum() * retryBudgetPercent / 100},
     * further retries are suppressed. This prevents retry storms under cascading failure
     * where every connection is retrying simultaneously. Inspired by Envoy's retry budget.
     */
    private final int retryBudgetPercent;

    /**
     * Connect timeout in milliseconds from the transport configuration.
     * Retained for future use (e.g., async retry scheduling) but not used for
     * blocking waits, which would deadlock when frontend and backend connections
     * share the same EventLoopGroup.
     */
    @SuppressWarnings("unused")
    private final long connectTimeoutMs;

    /**
     * Create a new {@link UpstreamRetryHandler}.
     *
     * @param httpLoadBalancer   the load balancer used to select backend nodes
     * @param bootstrapper       the bootstrapper used to create backend connections
     * @param maxRetries         maximum number of retry attempts (must be >= 0)
     * @param connectTimeoutMs   backend connect timeout in milliseconds
     * @param retryBudgetPercent maximum percentage of requests that may be retries (0-100)
     */
    UpstreamRetryHandler(HTTPLoadBalancer httpLoadBalancer, Bootstrapper bootstrapper,
                         int maxRetries, long connectTimeoutMs, int retryBudgetPercent) {
        if (maxRetries < 0) {
            throw new IllegalArgumentException("maxRetries must be >= 0, got: " + maxRetries);
        }
        if (retryBudgetPercent < 0 || retryBudgetPercent > 100) {
            throw new IllegalArgumentException("retryBudgetPercent must be 0-100, got: " + retryBudgetPercent);
        }
        this.httpLoadBalancer = httpLoadBalancer;
        this.bootstrapper = bootstrapper;
        this.maxRetries = maxRetries;
        this.connectTimeoutMs = connectTimeoutMs;
        this.retryBudgetPercent = retryBudgetPercent;
    }

    /**
     * Create a new {@link UpstreamRetryHandler} with settings from {@link com.shieldblaze.expressgateway.configuration.http.RetryConfiguration}.
     *
     * @param httpLoadBalancer the load balancer used to select backend nodes
     * @param bootstrapper     the bootstrapper used to create backend connections
     */
    UpstreamRetryHandler(HTTPLoadBalancer httpLoadBalancer, Bootstrapper bootstrapper) {
        this(httpLoadBalancer, bootstrapper, DEFAULT_MAX_RETRIES,
                httpLoadBalancer.configurationContext().transportConfiguration().backendConnectTimeout(),
                httpLoadBalancer.httpConfiguration().retryConfiguration().retryBudgetPercent());
    }

    /**
     * Attempt to establish a backend connection with retry logic for idempotent HTTP/1.1 requests.
     * <p>
     * If the initial connection to {@code initialNode} fails with a connection-level error
     * and the request method is idempotent, this method retries on different backend nodes
     * up to {@link #maxRetries} times.
     *
     * @param cluster       the cluster to select alternative nodes from
     * @param initialNode   the first node to attempt connection to
     * @param clientChannel the client-side channel (upstream)
     * @param request       the HTTP/1.1 request being proxied
     * @param socketAddress the client's socket address for load balancer request context
     * @return a {@link RetryResult} containing the connection and the node it connected to,
     *         or {@code null} if all attempts failed
     */
    RetryResult attemptWithRetry(Cluster cluster, Node initialNode, Channel clientChannel,
                                 HttpRequest request, InetSocketAddress socketAddress,
                                 ConnectionPool pool) {
        totalRequests.increment();

        boolean isIdempotent = isIdempotentMethod(request.method());
        Set<Node> triedNodes = new HashSet<>();
        triedNodes.add(initialNode);

        StandardEdgeNetworkMetricRecorder.INSTANCE.recordPoolMiss();
        HttpConnection connection = bootstrapper.create(initialNode, clientChannel, pool);

        // Add a listener on the connect future to detect connection failures.
        // If the connection succeeds, we return immediately. If it fails and
        // the method is idempotent, we try alternative nodes.
        if (!isConnectionFailure(connection)) {
            return new RetryResult(connection, initialNode);
        }

        // Connection to initial node failed — only retry for idempotent methods.
        if (!isIdempotent) {
            logger.warn("Connection to {} failed for non-idempotent method {}, not retrying",
                    initialNode.socketAddress(), request.method());
            return null;
        }

        logger.info("Connection to {} failed, attempting retry for idempotent {} request",
                initialNode.socketAddress(), request.method());

        for (int attempt = 0; attempt < maxRetries; attempt++) {
            // RES-BUDGET: Check retry budget before each attempt. If retries exceed
            // the configured percentage of total requests, skip the retry to prevent
            // a retry storm from amplifying a cascading failure. This is the same
            // concept as Envoy's retry budget (envoyproxy/envoy#4526).
            if (isRetryBudgetExhausted()) {
                logger.warn("Retry budget exhausted ({}% of {} total requests), skipping retry for {} {}",
                        retryBudgetPercent, totalRequests.sum(), request.method(), sanitize(request.uri()));
                return null;
            }
            totalRetries.increment();

            StandardEdgeNetworkMetricRecorder.INSTANCE.recordRetryAttempt();
            Node nextNode = selectAlternativeNode(cluster, socketAddress, request.headers(), triedNodes);
            if (nextNode == null) {
                logger.warn("No alternative backend node available for retry (attempt {}/{})",
                        attempt + 1, maxRetries);
                break;
            }

            triedNodes.add(nextNode);
            logger.info("Retry attempt {}/{}: trying node {} for {} {}",
                    attempt + 1, maxRetries, nextNode.socketAddress(), request.method(), sanitize(request.uri()));

            connection = bootstrapper.create(nextNode, clientChannel, pool);
            if (!isConnectionFailure(connection)) {
                return new RetryResult(connection, nextNode);
            }

            logger.warn("Retry attempt {}/{} to {} also failed",
                    attempt + 1, maxRetries, nextNode.socketAddress());
        }

        // All retries exhausted
        logger.error("All {} retry attempts exhausted for {} {}", maxRetries, request.method(), sanitize(request.uri()));
        return null;
    }

    /**
     * Attempt to establish a backend connection with retry logic for idempotent HTTP/2 requests.
     * <p>
     * Same semantics as the HTTP/1.1 variant but uses HTTP/2 headers to determine
     * the request method for idempotency checking.
     *
     * @param cluster       the cluster to select alternative nodes from
     * @param initialNode   the first node to attempt connection to
     * @param clientChannel the client-side channel (upstream)
     * @param headers       the HTTP/2 headers of the request being proxied
     * @param socketAddress the client's socket address for load balancer request context
     * @return a {@link RetryResult} containing the connection and the node it connected to,
     *         or {@code null} if all attempts failed
     */
    RetryResult attemptWithRetryH2(Cluster cluster, Node initialNode, Channel clientChannel,
                                   Http2Headers headers, InetSocketAddress socketAddress,
                                   ConnectionPool pool) {
        totalRequests.increment();

        CharSequence methodSeq = headers.method();
        boolean isIdempotent = methodSeq != null && isIdempotentMethod(HttpMethod.valueOf(methodSeq.toString()));
        Set<Node> triedNodes = new HashSet<>();
        triedNodes.add(initialNode);

        StandardEdgeNetworkMetricRecorder.INSTANCE.recordPoolMiss();
        HttpConnection connection = bootstrapper.create(initialNode, clientChannel, pool);

        if (!isConnectionFailure(connection)) {
            return new RetryResult(connection, initialNode);
        }

        if (!isIdempotent) {
            logger.warn("Connection to {} failed for non-idempotent method {}, not retrying",
                    initialNode.socketAddress(), sanitize(methodSeq));
            return null;
        }

        logger.info("Connection to {} failed, attempting retry for idempotent {} request",
                initialNode.socketAddress(), sanitize(methodSeq));

        for (int attempt = 0; attempt < maxRetries; attempt++) {
            // RES-BUDGET: Check retry budget before each attempt (see attemptWithRetry for rationale).
            if (isRetryBudgetExhausted()) {
                logger.warn("Retry budget exhausted ({}% of {} total requests), skipping retry for {} {}",
                        retryBudgetPercent, totalRequests.sum(), sanitize(methodSeq), sanitize(headers.path()));
                return null;
            }
            totalRetries.increment();

            StandardEdgeNetworkMetricRecorder.INSTANCE.recordRetryAttempt();
            Node nextNode = selectAlternativeNodeH2(cluster, socketAddress, headers, triedNodes);
            if (nextNode == null) {
                logger.warn("No alternative backend node available for retry (attempt {}/{})",
                        attempt + 1, maxRetries);
                break;
            }

            triedNodes.add(nextNode);
            logger.info("Retry attempt {}/{}: trying node {} for {} {}",
                    attempt + 1, maxRetries, nextNode.socketAddress(), sanitize(methodSeq), sanitize(headers.path()));

            connection = bootstrapper.create(nextNode, clientChannel, pool);
            if (!isConnectionFailure(connection)) {
                return new RetryResult(connection, nextNode);
            }

            logger.warn("Retry attempt {}/{} to {} also failed",
                    attempt + 1, maxRetries, nextNode.socketAddress());
        }

        logger.error("All {} retry attempts exhausted for {} {}", maxRetries, sanitize(methodSeq), sanitize(headers.path()));
        return null;
    }

    /**
     * Check whether the global retry budget has been exhausted.
     *
     * <p>The budget is expressed as a percentage: if retries exceed
     * {@code retryBudgetPercent}% of total requests, further retries are suppressed.
     * This prevents retry amplification during cascading failures — without a budget,
     * if 50% of backends are down, every request generates a retry, doubling the load
     * on the remaining healthy backends and potentially causing a full cascade.</p>
     *
     * <p>LongAdder.sum() provides an eventually-consistent snapshot. Under contention,
     * the sum may be slightly stale, which is acceptable: the budget is a safety valve
     * with some tolerance, not an exact accounting system. The slight inaccuracy is
     * preferable to the contention cost of an AtomicLong CAS loop on every request.</p>
     *
     * @return {@code true} if the retry budget is exhausted and retries should be skipped
     */
    private boolean isRetryBudgetExhausted() {
        long requests = totalRequests.sum();
        long retries = totalRetries.sum();
        // Avoid division: retries > requests * percent / 100 is equivalent to
        // retries * 100 > requests * percent, but use long multiplication to avoid overflow
        // for extremely large values. At 10M req/s running for hours this could overflow
        // a long multiplication, so use the division form which is safe.
        return requests > 0 && retries > requests * retryBudgetPercent / 100;
    }

    /**
     * Check if the connection's ChannelFuture completed with a connection-level failure.
     * <p>
     * CM-02: Uses a non-blocking check to avoid deadlocking the EventLoop. The previous
     * implementation used {@code awaitUninterruptibly()} which could deadlock when
     * frontend and backend connections share the same EventLoopGroup (childGroup).
     * If all EventLoop threads are blocked in {@code awaitUninterruptibly()}, no thread
     * is available to complete the backend TCP handshake, causing a full deadlock.
     * <p>
     * Instead, we check {@code isDone()} non-blockingly. If the connect hasn't completed
     * yet, we treat it as "not failed" and let the backlog mechanism in {@link Connection}
     * handle queued writes. If the connect ultimately fails, the backlog is cleared and
     * the connection closes (triggering {@link DownstreamHandler#channelInactive}).
     *
     * @return {@code true} if the connection failed with a retryable error, {@code false} if it succeeded or is still in progress
     */
    private boolean isConnectionFailure(HttpConnection connection) {
        // Non-blocking check: if the connect future hasn't completed yet, the
        // connection is still in progress. Return false (not a failure) and let
        // the backlog mechanism handle it. The connect will complete on its own
        // EventLoop thread without blocking the current one.
        if (!connection.channelFuture().isDone()) {
            return false;
        }

        if (connection.channelFuture().isSuccess()) {
            return false;
        }

        Throwable cause = connection.channelFuture().cause();
        // Only retry on connection-level failures, not protocol errors.
        // ConnectException: OS-level "connection refused" (ECONNREFUSED).
        // ConnectTimeoutException (extends ConnectException): Netty's connect timeout
        // fired before TCP handshake completed. Covered by the ConnectException check.
        return cause instanceof ConnectException;
    }

    /**
     * Select an alternative node from the cluster, excluding nodes already tried.
     * Returns {@code null} if no suitable alternative is available.
     */
    private Node selectAlternativeNode(Cluster cluster, InetSocketAddress socketAddress,
                                       io.netty.handler.codec.http.HttpHeaders headers,
                                       Set<Node> triedNodes) {
        try {
            // The load balancer may return a node we already tried (e.g., round-robin
            // with only 2 nodes). We attempt up to cluster.onlineNodes().size() times
            // to find a fresh node to avoid infinite loops.
            int maxAttempts = cluster.onlineNodes().size();
            for (int i = 0; i < maxAttempts; i++) {
                Node candidate = cluster.nextNode(new HTTPBalanceRequest(socketAddress, headers)).node();
                if (!triedNodes.contains(candidate) && !candidate.connectionFull()) {
                    return candidate;
                }
            }
        } catch (Exception e) {
            logger.error("Error selecting alternative node for retry", e);
        }
        return null;
    }

    /**
     * Select an alternative node for HTTP/2, excluding nodes already tried.
     */
    private Node selectAlternativeNodeH2(Cluster cluster, InetSocketAddress socketAddress,
                                         Http2Headers headers, Set<Node> triedNodes) {
        try {
            int maxAttempts = cluster.onlineNodes().size();
            for (int i = 0; i < maxAttempts; i++) {
                Node candidate = cluster.nextNode(new HTTPBalanceRequest(socketAddress, headers)).node();
                if (!triedNodes.contains(candidate) && !candidate.connectionFull()) {
                    return candidate;
                }
            }
        } catch (Exception e) {
            logger.error("Error selecting alternative node for retry", e);
        }
        return null;
    }

    /**
     * Determine if the given HTTP method is idempotent per RFC 9110 Section 9.2.2.
     * <p>
     * Idempotent methods: GET, HEAD, PUT, DELETE, OPTIONS, TRACE.
     * Non-idempotent methods: POST, PATCH, CONNECT.
     */
    static boolean isIdempotentMethod(HttpMethod method) {
        return HttpMethod.GET.equals(method)
                || HttpMethod.HEAD.equals(method)
                || HttpMethod.PUT.equals(method)
                || HttpMethod.DELETE.equals(method)
                || HttpMethod.OPTIONS.equals(method)
                || HttpMethod.TRACE.equals(method);
    }

    /**
     * Result of a retry attempt, containing the successfully established connection
     * and the node it connected to.
     */
    record RetryResult(HttpConnection connection, Node node) {
    }
}
