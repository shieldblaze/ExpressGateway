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
package com.shieldblaze.expressgateway.protocol.http.http3;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBytesTracker;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.core.factory.BootstrapFactory;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.ConnectionPool;
import com.shieldblaze.expressgateway.protocol.http.HttpConnection;
import io.netty.bootstrap.Bootstrap;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.socket.SocketChannel;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpClientCodec;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http3.DefaultHttp3Headers;
import io.netty.handler.codec.http3.DefaultHttp3HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;
import io.netty.handler.codec.http3.Http3RequestStreamInboundHandler;
import io.netty.handler.codec.quic.QuicChannel;
import io.netty.handler.codec.quic.QuicStreamChannel;
import io.netty.handler.timeout.ReadTimeoutHandler;
import io.netty.util.AttributeKey;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.TimeUnit;

/**
 * HTTP/3 request stream handler. One instance per bidirectional QUIC stream.
 *
 * <p>Processes incoming HTTP/3 frames (HEADERS, DATA) on a single request stream,
 * translates them to HTTP/1.1 objects for backend forwarding, performs load balancing,
 * and proxies backend responses back as HTTP/3 frames.</p>
 *
 * <p>Per RFC 9114 Section 4.1, an HTTP/3 request message consists of:
 * <ol>
 *   <li>A single HEADERS frame containing the request headers (pseudo-headers + regular headers)</li>
 *   <li>Zero or more DATA frames containing the request body</li>
 *   <li>Optionally, a trailing HEADERS frame containing trailers</li>
 * </ol>
 *
 * <p>Unlike HTTP/2, there is no stream ID in the frame -- the QUIC stream itself provides
 * the stream identity. Each request stream is independent (no head-of-line blocking).
 * This handler is bound to a single {@link QuicStreamChannel}.</p>
 *
 * <p>Backend connections are acquired from a {@link ConnectionPool} and use
 * {@link HttpConnection}'s backlog queue to safely buffer DATA frames that arrive
 * before the TCP backend connect completes. This prevents silent data loss.</p>
 *
 * <p>The handler enforces:
 * <ul>
 *   <li>Method whitelist (from {@link HttpConfiguration})</li>
 *   <li>Request body size limits</li>
 *   <li>Pseudo-header validation per RFC 9114 Section 4.3</li>
 *   <li>CRLF injection defense (when converting to HTTP/1.1)</li>
 *   <li>Proper HTTP/3 error codes on stream closure (RFC 9114 Section 8.1)</li>
 * </ul>
 */
@SuppressWarnings("ConfusingOverload") // channelRead overloads are required by Netty's Http3RequestStreamInboundHandler
final class Http3ServerHandler extends Http3RequestStreamInboundHandler {

    private static final Logger logger = LogManager.getLogger(Http3ServerHandler.class);

    /**
     * H3_MESSAGE_ERROR (0x10e): Malformed request (bad headers, CRLF injection, etc.)
     * per RFC 9114 Section 8.1.
     */
    private static final long H3_MESSAGE_ERROR = 0x10e;

    /**
     * H3_REQUEST_CANCELLED (0x10c): Used when cancelling a request after sending
     * an error response (e.g., 413 body too large) per RFC 9114 Section 8.1.
     */
    private static final long H3_REQUEST_CANCELLED = 0x10c;

    /**
     * H3_INTERNAL_ERROR (0x102): Used for internal server errors per RFC 9114 Section 8.1.
     */
    private static final long H3_INTERNAL_ERROR = 0x102;

    /**
     * H3_STREAM_CREATION_ERROR (0x103): A stream required by the connection was not created
     * per RFC 9114 Section 8.1.
     */
    private static final long H3_STREAM_CREATION_ERROR = 0x103;

    /**
     * H3_CLOSED_CRITICAL_STREAM (0x104): A stream required by the connection was closed or
     * reset per RFC 9114 Section 8.1. Used when a critical unidirectional stream (control,
     * QPACK encoder, QPACK decoder) is unexpectedly closed.
     */
    private static final long H3_CLOSED_CRITICAL_STREAM = 0x104;

    /**
     * H3_EXCESSIVE_LOAD (0x107): The endpoint detected that its peer is generating an
     * excessive load per RFC 9114 Section 8.1. Analogous to HTTP/2 ENHANCE_YOUR_CALM.
     * Used when a connection exceeds frame or stream creation rate limits.
     */
    private static final long H3_EXCESSIVE_LOAD = 0x107;

    /**
     * Maximum number of DATA frames allowed per stream before the stream is considered
     * abusive. This defends against a client sending many tiny DATA frames on a single
     * stream to amplify per-frame processing overhead (a variant of the "data dribble" attack).
     *
     * <p>With a typical 16 KB MTU and a 10 MB max body, ~640 frames is normal.
     * The base limit is 10,000 frames per stream, but scales up with maxRequestBodySize
     * to avoid false positives for large uploads (using 1 KB as the minimum expected frame
     * size for worst-case QUIC congestion window fragmentation).</p>
     */
    private static final int BASE_MAX_DATA_FRAMES = 10_000;
    private static final int MIN_EXPECTED_FRAME_SIZE = 1024;

    // ─── HTTP/3 Graceful Shutdown (RFC 9114 Section 5.2) ─────────────────────
    // HTTP/3 graceful shutdown uses the QUIC connection's GOAWAY mechanism.
    // The "draining" state is stored as a Boolean attribute on the parent QuicChannel
    // so that all stream handlers on the same connection can observe it.
    //
    // When startDraining() is called on a QuicChannel:
    //   1. The DRAINING_KEY attribute is set to true
    //   2. New streams created after this point will see draining=true in their
    //      channelRead(HEADERS) and respond with 503 + close
    //   3. Existing streams continue to completion
    //   4. The caller (Http3LoadBalancer.stop()) sends QUIC GOAWAY and schedules
    //      a hard close after the drain timeout

    /**
     * Attribute key stored on the parent {@link QuicChannel} to signal that the connection
     * is in graceful shutdown mode. Delegates to {@link Http3Constants#DRAINING_KEY}.
     */
    static final AttributeKey<Boolean> DRAINING_KEY = Http3Constants.DRAINING_KEY;

    /**
     * Initiates graceful shutdown (draining) on an HTTP/3 connection per RFC 9114 Section 5.2.
     *
     * <p>After calling this method, new request streams on the connection will be rejected
     * with 503 Service Unavailable. Existing streams are allowed to complete normally.
     * The caller is responsible for sending QUIC GOAWAY and scheduling the hard close
     * (see {@link Http3LoadBalancer#stop()} for the full shutdown sequence).</p>
     *
     * <p>Must be called on the QuicChannel's EventLoop thread or with an execute() wrapper.</p>
     *
     * @param quicChannel the parent QUIC connection channel to drain
     */
    static void startDraining(QuicChannel quicChannel) {
        quicChannel.attr(DRAINING_KEY).set(Boolean.TRUE);
        logger.debug("H3-DRAIN: Marked QUIC connection {} as draining", quicChannel);
    }

    /**
     * Checks whether the parent QUIC connection is in draining mode.
     *
     * @param ctx the stream's channel handler context
     * @return {@code true} if the connection is draining
     */
    private static boolean isConnectionDraining(ChannelHandlerContext ctx) {
        Channel ch = ctx.channel();
        if (ch.parent() instanceof QuicChannel quicChannel) {
            Boolean draining = quicChannel.attr(DRAINING_KEY).get();
            return Boolean.TRUE.equals(draining);
        }
        return false;
    }

    private final L4LoadBalancer loadBalancer;
    private final Channel frontendStreamChannel;
    private final ConnectionPool connectionPool;

    /**
     * The pooled backend connection for this stream's request.
     * Wraps the raw channel and provides backlog queueing -- writes are
     * buffered if the channel is not yet active, preventing DATA frame drops.
     */
    private HttpConnection httpConnection;

    /**
     * The backend Node selected by the load balancer for this request.
     * Stored so the downstream handler can return the connection to the pool
     * when the response completes.
     */
    private Node selectedNode;

    /**
     * Accumulated request body bytes on this stream. Used to enforce the
     * per-request body size limit from HttpConfiguration.
     */
    private long bodyBytesReceived;

    /**
     * Maximum request body size in bytes. Cached from HttpConfiguration
     * at construction time to avoid repeated lookups on the hot path.
     */
    private final long maxRequestBodySize;

    /**
     * Allowed HTTP methods. Cached from HttpConfiguration.
     */
    private final Set<String> allowedMethods;

    /**
     * Timestamp (nanoTime) when the request headers were received.
     * Used to compute end-to-end latency when the response completes.
     */
    private long requestStartNanos;

    /**
     * Set to {@code true} after the initial HEADERS frame has been processed.
     * Any subsequent HEADERS frame on this stream is treated as trailers
     * per RFC 9114 Section 4.1.
     */
    private boolean initialHeadersReceived;

    /**
     * Set to {@code true} after we send an error response on this stream.
     * Prevents sending multiple error responses and short-circuits subsequent frames.
     */
    private boolean errorSent;

    /**
     * Set to {@code true} after trailers have been forwarded to the backend
     * via {@link #processTrailers}. Prevents {@link #channelInputClosed} from
     * sending a duplicate {@link LastHttpContent#EMPTY_LAST_CONTENT}.
     */
    private boolean trailersSent;

    /**
     * Per-stream DATA frame counter. Tracks the number of DATA frames received
     * on this stream for excessive-load detection per RFC 9114 Section 8.1.
     * An abnormally high frame count (many tiny DATA frames) indicates a data-dribble
     * attack — the client is maximizing per-frame processing overhead. When exceeded,
     * the stream is reset with H3_EXCESSIVE_LOAD (0x107).
     */
    private int dataFrameCount;

    /**
     * Dynamic per-stream DATA frame limit, scaled by maxRequestBodySize.
     */
    private final int maxDataFramesPerStream;

    Http3ServerHandler(L4LoadBalancer loadBalancer, Channel frontendStreamChannel,
                       ConnectionPool connectionPool) {
        this.loadBalancer = loadBalancer;
        this.frontendStreamChannel = frontendStreamChannel;
        this.connectionPool = connectionPool;

        HttpConfiguration httpConfig = loadBalancer.configurationContext().httpConfiguration();
        this.maxRequestBodySize = httpConfig.maxRequestBodySize();
        this.allowedMethods = httpConfig.allowedMethods();

        // Scale frame limit with configured max body size to avoid false positives
        // for large uploads while still defending against data-dribble attacks.
        if (maxRequestBodySize > 0) {
            this.maxDataFramesPerStream = (int) Math.max(BASE_MAX_DATA_FRAMES,
                    maxRequestBodySize / MIN_EXPECTED_FRAME_SIZE);
        } else {
            this.maxDataFramesPerStream = BASE_MAX_DATA_FRAMES;
        }
    }

    // -- HTTP/3 HEADERS frame handling -------------------------------------------

    @Override
    protected void channelRead(ChannelHandlerContext ctx, Http3HeadersFrame headersFrame) throws Exception {
        if (errorSent) {
            ReferenceCountUtil.release(headersFrame);
            return;
        }

        if (!initialHeadersReceived) {
            // First HEADERS frame: request headers
            initialHeadersReceived = true;
            requestStartNanos = System.nanoTime();
            processRequestHeaders(ctx, headersFrame);
        } else {
            // Subsequent HEADERS frame: trailers per RFC 9114 Section 4.1
            processTrailers(headersFrame);
        }
    }

    /**
     * Processes the initial request HEADERS frame. Validates pseudo-headers per
     * RFC 9114 Section 4.3, enforces method whitelist, performs load balancing,
     * and establishes a backend connection via the connection pool.
     *
     * <p>RFC 9114 Section 4.3.1 requires the following pseudo-headers on requests:
     * <ul>
     *   <li>{@code :method} -- always required</li>
     *   <li>{@code :scheme} -- required for non-CONNECT</li>
     *   <li>{@code :path} -- required for non-CONNECT, MUST start with "/" (or "*" for OPTIONS)</li>
     *   <li>{@code :authority} -- required for http/https URIs</li>
     * </ul>
     */
    private void processRequestHeaders(ChannelHandlerContext ctx, Http3HeadersFrame headersFrame) {
        // -- RFC 9114 Section 5.2: Reject new streams during graceful shutdown ------
        // When the connection is draining, respond 503 and close the stream normally.
        // Existing streams (those that already passed this check) continue unaffected.
        if (isConnectionDraining(ctx)) {
            logger.debug("H3-DRAIN: Rejecting new stream during graceful shutdown");
            sendErrorResponse(ctx, "503", -1, headersFrame);
            return;
        }

        Http3Headers h3Headers = headersFrame.headers();

        // GAP-H3-01: RFC 9114 Section 4.3 — pseudo-headers MUST appear before regular headers
        boolean seenRegularHeader = false;
        for (Map.Entry<CharSequence, CharSequence> entry : h3Headers) {
            CharSequence name = entry.getKey();
            if (name.length() > 0 && name.charAt(0) == ':') {
                if (seenRegularHeader) {
                    logger.warn("H3-REQ-04: Pseudo-header '{}' after regular header, rejecting", name);
                    sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                    return;
                }
            } else {
                seenRegularHeader = true;
            }
        }

        // GAP-H3-02: RFC 9114 Section 4.3 — :status is a response-only pseudo-header
        if (h3Headers.status() != null) {
            logger.warn("H3-REQ-05: :status pseudo-header in request, rejecting");
            sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
            return;
        }

        // -- Validate pseudo-headers per RFC 9114 Section 4.3 ----------------------
        CharSequence method = h3Headers.method();
        CharSequence scheme = h3Headers.scheme();
        CharSequence path = h3Headers.path();
        CharSequence authority = h3Headers.authority();

        if (method == null) {
            logger.warn("H3-REQ-01: Missing required :method pseudo-header on stream");
            sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
            return;
        }

        boolean isConnect = "CONNECT".contentEquals(method);

        if (!isConnect) {
            if (scheme == null) {
                logger.warn("H3-REQ-01: Missing required :scheme pseudo-header on stream");
                sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                return;
            }
            if (path == null || path.length() == 0) {
                logger.warn("H3-REQ-01: Missing or empty :path pseudo-header on stream");
                sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                return;
            }
            if (authority == null) {
                logger.warn("H3-REQ-02: Missing :authority pseudo-header on non-CONNECT request");
                sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                return;
            }
        }

        // -- CRLF injection defense ------------------------------------------------
        // RFC 9114 Section 4.2: field values MUST NOT contain NUL, CR, or LF.
        // When translating HTTP/3 to HTTP/1.1 for backend forwarding, CRLF in
        // :path would inject headers into the backend request-line.
        if (containsProhibitedChars(path) || containsProhibitedChars(authority) || containsProhibitedChars(scheme)) {
            logger.warn("SEC-H3-01: CRLF injection attempt in pseudo-header on stream");
            sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
            return;
        }

        // -- :authority / Host consistency ------------------------------------------
        // RFC 9114 Section 4.3.1: if both :authority and Host are present, they
        // MUST have the same value. Divergence is a request smuggling vector.
        CharSequence host = h3Headers.get("host");
        if (!isConnect && authority != null && host != null) {
            if (!authority.toString().equalsIgnoreCase(host.toString())) {
                logger.warn("SEC-H3-02: :authority '{}' and Host '{}' differ, rejecting with 400",
                        authority, host);
                sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                return;
            }
        }

        // -- :path format validation -----------------------------------------------
        // RFC 9114 Section 4.3.1: :path MUST start with "/" for non-CONNECT,
        // except OPTIONS which may use asterisk-form "*".
        if (!isConnect && path != null) {
            boolean isOptions = "OPTIONS".contentEquals(method);
            boolean isAsteriskForm = path.length() == 1 && path.charAt(0) == '*';
            if (!(isOptions && isAsteriskForm) && path.charAt(0) != '/') {
                logger.warn("H3-REQ-03: Invalid :path '{}', must start with '/'", path);
                sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
                return;
            }
        }

        // -- Reject CONNECT and TRACE ----------------------------------------------
        // CONNECT has special semantics not implemented here.
        // TRACE is a security risk to proxy (RFC 9110 Section 9.3.8).
        String methodStr = method.toString();
        if (HttpMethod.CONNECT.name().equalsIgnoreCase(methodStr)
                || HttpMethod.TRACE.name().equalsIgnoreCase(methodStr)) {
            logger.debug("Rejecting {} method on HTTP/3 stream", methodStr);
            sendErrorResponse(ctx, "405", H3_MESSAGE_ERROR, headersFrame);
            return;
        }

        // -- Method whitelist enforcement ------------------------------------------
        if (!allowedMethods.contains(methodStr)) {
            logger.debug("Rejecting disallowed method {} on HTTP/3 stream", methodStr);
            sendErrorResponse(ctx, "405", H3_MESSAGE_ERROR, headersFrame);
            return;
        }

        // -- Load balancing --------------------------------------------------------
        // authority is guaranteed non-null: !isConnect → null guard at line 394, isConnect → rejected at line 441
        String authorityStr = Objects.requireNonNull(authority, "authority").toString();
        Cluster cluster;
        try {
            cluster = loadBalancer.cluster(authorityStr);
        } catch (Exception _) {
            cluster = null;
        }

        if (cluster == null) {
            logger.debug("No cluster found for authority '{}', responding 503", authorityStr);
            sendErrorResponse(ctx, "503", -1, headersFrame);
            return;
        }

        InetSocketAddress clientAddress = resolveClientAddress(ctx);

        // Build HttpHeaders for the load balancer request. HTTPBalanceRequest accepts
        // HttpHeaders (HTTP/1.1) or Http2Headers, but not Http3Headers directly.
        // We construct a lightweight HttpHeaders with the key routing headers.
        HttpHeaders balanceHeaders = new DefaultHttpHeaders();
        balanceHeaders.set(HttpHeaderNames.HOST, authority.toString());
        // Copy non-pseudo headers for balancing strategies that inspect custom headers
        for (Map.Entry<CharSequence, CharSequence> entry : h3Headers) {
            CharSequence headerName = entry.getKey();
            if (headerName.length() > 0 && headerName.charAt(0) != ':') {
                balanceHeaders.add(headerName.toString(), entry.getValue().toString());
            }
        }

        Node node;
        try {
            node = cluster.nextNode(new HTTPBalanceRequest(clientAddress, balanceHeaders)).node();
        } catch (Exception e) {
            logger.error("Load balancing failed for authority '{}'", authorityStr, e);
            sendErrorResponse(ctx, "503", -1, headersFrame);
            return;
        }

        if (node == null || node.connectionFull()) {
            logger.debug("Backend node unavailable or at capacity for authority '{}'", authorityStr);
            sendErrorResponse(ctx, "503", -1, headersFrame);
            return;
        }

        selectedNode = node;

        // -- Translate HTTP/3 HEADERS to HTTP/1.1 request --------------------------
        // The gateway proxies HTTP/3 frontend to HTTP/1.1 backend by default.
        // RFC 9110 Section 3.7: intermediaries can translate between protocol versions.
        // path is guaranteed non-null: !isConnect → null guard at line 389, isConnect → rejected at line 441
        String pathStr = Objects.requireNonNull(path, "path").toString();

        // DEF-H3-URI: URI normalization for HTTP/3 — parity with HTTP/1.1 handler.
        // Applies RFC 3986 Section 5.2.4 dot-segment removal, null byte rejection,
        // double-encode detection, and path traversal defense.
        String normalizedPath = com.shieldblaze.expressgateway.protocol.http.UriNormalizer.normalizeUri(pathStr);
        if (normalizedPath == null) {
            logger.warn("SEC-H3-URI: Path traversal or prohibited pattern in :path '{}', rejecting with 400", pathStr);
            sendErrorResponse(ctx, "400", H3_MESSAGE_ERROR, headersFrame);
            return;
        }
        pathStr = normalizedPath;

        HttpMethod httpMethod = HttpMethod.valueOf(methodStr);
        DefaultHttpRequest httpRequest = new DefaultHttpRequest(HttpVersion.HTTP_1_1, httpMethod, pathStr);

        // Copy regular headers, stripping hop-by-hop and HTTP/3-specific headers
        for (Map.Entry<CharSequence, CharSequence> entry : h3Headers) {
            CharSequence name = entry.getKey();
            // Skip pseudo-headers (start with ':')
            if (name.length() > 0 && name.charAt(0) == ':') {
                continue;
            }
            // Skip hop-by-hop headers not applicable to HTTP/1.1 backend
            String nameLower = name.toString().toLowerCase();
            if ("connection".equals(nameLower) || "keep-alive".equals(nameLower)
                    || "transfer-encoding".equals(nameLower) || "upgrade".equals(nameLower)
                    || "te".equals(nameLower) || "proxy-connection".equals(nameLower)) {
                continue;
            }
            httpRequest.headers().add(name.toString(), entry.getValue().toString());
        }

        // Set Host header from :authority (required for HTTP/1.1 backend)
        if (!httpRequest.headers().contains(HttpHeaderNames.HOST)) {
            httpRequest.headers().set(HttpHeaderNames.HOST, authorityStr);
        }

        // -- Inject X-Forwarded-* proxy headers for backend visibility -------------
        if (clientAddress != null) {
            httpRequest.headers().set("X-Forwarded-For", clientAddress.getAddress().getHostAddress());
        }
        httpRequest.headers().set("X-Forwarded-Proto", scheme != null ? scheme.toString() : "https");
        httpRequest.headers().set("X-Forwarded-Host", authorityStr);
        // DEF-H3-02: Set X-Forwarded-Port for consistency with HTTP/1.1 and HTTP/2 paths.
        // Use the local address of the parent QUIC channel (the proxy's listening port).
        java.net.InetSocketAddress localAddr = resolveLocalAddress(ctx);
        if (localAddr != null) {
            httpRequest.headers().set("X-Forwarded-Port", String.valueOf(localAddr.getPort()));
        }

        // GAP-H3-07: RFC 9110 Section 7.6.3 — proxy MUST add Via header
        httpRequest.headers().add("Via", "3.0 expressgateway");

        // -- Acquire or create backend connection from pool ------------------------
        // Try to acquire an idle H1 connection from the pool first.
        // If none available, create a new one via Bootstrapper pattern.
        HttpConnection pooledConn = connectionPool.acquireH1(node);
        if (pooledConn != null) {
            // Reuse pooled connection -- replace the downstream handler to point
            // at this stream's frontend channel.
            httpConnection = pooledConn;
            replaceDownstreamHandler(httpConnection);

            // Connection is already active; write immediately.
            httpConnection.writeAndFlush(httpRequest);
        } else {
            // No pooled connection available -- create a new one.
            // HttpConnection's backlog queue will buffer the httpRequest (and any
            // subsequent DATA frames) until the TCP connect completes.
            HttpConfiguration httpConfig = loadBalancer.configurationContext().httpConfiguration();
            httpConnection = new HttpConnection(node, httpConfig);

            Bootstrap bootstrap = BootstrapFactory.tcp(
                    loadBalancer.configurationContext(),
                    loadBalancer.eventLoopFactory().childGroup(),
                    loadBalancer.byteBufAllocator()
            );

            bootstrap.handler(new ChannelInitializer<SocketChannel>() {
                @Override
                protected void initChannel(SocketChannel ch) {
                    ChannelPipeline pipeline = ch.pipeline();

                    pipeline.addFirst(new NodeBytesTracker(node));

                    Duration timeout = Duration.ofMillis(
                            loadBalancer.configurationContext().transportConfiguration().connectionIdleTimeout());
                    pipeline.addLast(new ConnectionTimeoutHandler(timeout, false));

                    pipeline.addLast(new ReadTimeoutHandler(
                            httpConfig.backendResponseTimeoutSeconds(), TimeUnit.SECONDS));

                    pipeline.addLast(new HttpClientCodec(
                            httpConfig.maxInitialLineLength(),
                            httpConfig.maxHeaderSize(),
                            httpConfig.maxChunkSize()));
                    pipeline.addLast(new HttpContentDecompressor(0));
                    pipeline.addLast(new Http3DownstreamHandler(
                            frontendStreamChannel, requestStartNanos,
                            connectionPool, node, httpConnection));
                }
            });

            ChannelFuture connectFuture = bootstrap.connect(node.socketAddress());
            httpConnection.init(connectFuture);

            try {
                node.addConnection(httpConnection);
            } catch (Exception e) {
                logger.warn("Failed to register backend connection with node {}: {}",
                        node.socketAddress(), e.getMessage());
                httpConnection.close();
                ReferenceCountedUtil.silentRelease(httpRequest);
                sendErrorResponse(ctx, "503", -1);
                ReferenceCountUtil.release(headersFrame);
                return;
            }

            // Write the request into the HttpConnection. If the channel is not yet
            // active (INITIALIZED state), this is queued in the backlog and flushed
            // automatically when the connection succeeds. NO DATA IS DROPPED.
            httpConnection.writeAndFlush(httpRequest);
        }

        ReferenceCountUtil.release(headersFrame);
    }

    /**
     * Replaces the downstream handler on an existing pooled connection's channel
     * to point at this stream's frontend channel for response forwarding.
     */
    private void replaceDownstreamHandler(HttpConnection conn) {
        Channel backendCh = conn.channel();
        if (backendCh != null && backendCh.isActive()) {
            Http3DownstreamHandler oldHandler = backendCh.pipeline().get(Http3DownstreamHandler.class);
            if (oldHandler != null) {
                backendCh.pipeline().replace(oldHandler, "http3Downstream",
                        new Http3DownstreamHandler(
                                frontendStreamChannel, requestStartNanos,
                                connectionPool, selectedNode, httpConnection));
            }
        }
    }

    /**
     * Processes trailer HEADERS frames. Trailers are forwarded to the backend
     * as {@link LastHttpContent} with trailing headers.
     */
    private void processTrailers(Http3HeadersFrame headersFrame) {
        trailersSent = true;
        if (httpConnection != null) {
            DefaultLastHttpContent lastContent = new DefaultLastHttpContent();
            for (Map.Entry<CharSequence, CharSequence> entry : headersFrame.headers()) {
                CharSequence name = entry.getKey();
                // Trailers MUST NOT include pseudo-headers (RFC 9114 Section 4.3)
                if (name.length() > 0 && name.charAt(0) == ':') {
                    continue;
                }
                lastContent.trailingHeaders().add(name.toString(), entry.getValue().toString());
            }
            // writeAndFlush queues to backlog if channel not yet active -- NO DROPS
            httpConnection.writeAndFlush(lastContent);
        }
        ReferenceCountUtil.release(headersFrame);
    }

    // -- HTTP/3 DATA frame handling ------------------------------------------------

    @Override
    protected void channelRead(ChannelHandlerContext ctx, Http3DataFrame dataFrame) throws Exception {
        if (errorSent) {
            ReferenceCountUtil.release(dataFrame);
            return;
        }

        ByteBuf content = dataFrame.content();
        int readableBytes = content.readableBytes();

        // -- RFC 9114 Section 8.1: Per-stream DATA frame rate limit -----------------
        // Detect data-dribble attacks where a client sends many tiny DATA frames to
        // amplify per-frame processing overhead. H3_EXCESSIVE_LOAD (0x107) is the
        // appropriate error code per RFC 9114 Section 8.1.
        dataFrameCount++;
        if (dataFrameCount > maxDataFramesPerStream) {
            logger.warn("H3-EXCESSIVE-LOAD: Stream received {} DATA frames (limit {}), resetting with H3_EXCESSIVE_LOAD",
                    dataFrameCount, maxDataFramesPerStream);
            ReferenceCountUtil.release(dataFrame);
            sendErrorResponse(ctx, "429", H3_EXCESSIVE_LOAD);
            closeBackendConnection();
            return;
        }

        // -- Enforce request body size limit ----------------------------------------
        bodyBytesReceived += readableBytes;
        if (maxRequestBodySize > 0 && bodyBytesReceived > maxRequestBodySize) {
            logger.warn("Request body size limit exceeded: {} > {} bytes",
                    bodyBytesReceived, maxRequestBodySize);
            ReferenceCountUtil.release(dataFrame);
            sendErrorResponse(ctx, "413", H3_REQUEST_CANCELLED);
            closeBackendConnection();
            return;
        }

        // Forward body data to backend as HttpContent.
        // httpConnection.writeAndFlush() queues to backlog if channel not yet active.
        // This fixes the silent DATA frame drop bug.
        if (httpConnection != null) {
            DefaultHttpContent httpContent = new DefaultHttpContent(content.retain());
            httpConnection.writeAndFlush(httpContent);

            // RB-BP-01: Proactive request-direction backpressure. After writing body
            // content to the backend, check if its outbound buffer has exceeded the
            // high water mark. If so, pause reading from the QUIC stream immediately
            // to prevent OOM when the backend is slower than the frontend.
            // This mirrors the BP-01 check in Http11ServerInboundHandler L731-733.
            Channel backendCh = httpConnection.channel();
            if (backendCh != null && !backendCh.isWritable()) {
                ctx.channel().config().setAutoRead(false);
            }
        }

        ReferenceCountUtil.release(dataFrame);
    }

    // -- Stream lifecycle ----------------------------------------------------------

    /**
     * Called when the QUIC stream's input side is closed (FIN received).
     * This signals the end of the request from the client. We send
     * {@link LastHttpContent} to the backend to complete the request.
     *
     * <p>If trailers were already forwarded via {@link #processTrailers},
     * we do NOT send a duplicate LastHttpContent -- the trailers already
     * carried the end-of-message signal.</p>
     */
    @Override
    protected void channelInputClosed(ChannelHandlerContext ctx) throws Exception {
        if (!trailersSent && httpConnection != null) {
            // writeAndFlush queues to backlog if channel not yet active -- NO DROPS
            httpConnection.writeAndFlush(LastHttpContent.EMPTY_LAST_CONTENT);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        // Stream closed -- clean up backend connection.
        closeBackendConnection();
        super.channelInactive(ctx);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Exception on HTTP/3 request stream", cause);
        if (!errorSent) {
            sendErrorResponse(ctx, "500", H3_INTERNAL_ERROR);
        }
        closeBackendConnection();
        ctx.close();
    }

    /**
     * Backpressure: when the frontend QUIC stream channel becomes unwritable,
     * pause reads on the backend channel to stop response data from accumulating
     * in the outbound buffer. Resume when the stream becomes writable again.
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        if (httpConnection != null) {
            Channel backendCh = httpConnection.channel();
            if (backendCh != null) {
                backendCh.config().setAutoRead(ctx.channel().isWritable());
            }
        }
        super.channelWritabilityChanged(ctx);
    }

    // -- Helper methods ------------------------------------------------------------

    /**
     * Sends an HTTP/3 error response on the current stream and marks the stream
     * as having sent an error to prevent duplicate responses.
     *
     * <p>Uses proper HTTP/3 error codes when closing the QUIC stream per
     * RFC 9114 Section 8.1:</p>
     * <ul>
     *   <li>Malformed request (bad headers): H3_MESSAGE_ERROR (0x10e)</li>
     *   <li>Body too large: H3_REQUEST_CANCELLED (0x10c) after 413 response</li>
     *   <li>Internal error: H3_INTERNAL_ERROR (0x102)</li>
     *   <li>Backend unreachable (502/503): send response then close normally (not a protocol error)</li>
     * </ul>
     *
     * @param ctx       the channel handler context
     * @param status    the HTTP status code as a string (e.g., "503")
     * @param h3Error   the HTTP/3 error code for QUIC stream reset, or -1 for normal close
     * @param toRelease optional frame to release after sending
     */
    private void sendErrorResponse(ChannelHandlerContext ctx, String status, long h3Error, Object... toRelease) {
        if (errorSent) {
            for (Object obj : toRelease) {
                ReferenceCountUtil.release(obj);
            }
            return;
        }
        errorSent = true;

        for (Object obj : toRelease) {
            ReferenceCountUtil.release(obj);
        }

        Http3Headers headers = new DefaultHttp3Headers();
        headers.status(status);
        headers.set("content-length", "0");
        Http3HeadersFrame responseFrame = new DefaultHttp3HeadersFrame(headers);

        if (h3Error >= 0) {
            // Protocol-level error: send the response, then reset the stream with
            // the appropriate HTTP/3 error code per RFC 9114 Section 8.1.
            // QuicStreamChannel.shutdown(int) sends RESET_STREAM with the given
            // application error code, which is the correct mechanism for HTTP/3
            // error signaling on individual request streams.
            ctx.writeAndFlush(responseFrame).addListener(f -> {
                if (ctx.channel() instanceof QuicStreamChannel quicStream) {
                    quicStream.shutdown((int) h3Error);
                } else {
                    ctx.close();
                }
            });
        } else {
            // Non-protocol error (502, 503): send response and close normally.
            // The QuicStreamChannel close sends FIN on the QUIC stream.
            ctx.writeAndFlush(responseFrame).addListener(ChannelFutureListener.CLOSE);
        }
    }

    /**
     * Closes the backend connection. If the connection was acquired from the pool,
     * it is evicted rather than returned (since this path is only reached on error
     * or stream cancellation).
     */
    private void closeBackendConnection() {
        if (httpConnection != null) {
            if (connectionPool != null && selectedNode != null) {
                connectionPool.evict(httpConnection);
            }
            httpConnection.close();
            httpConnection = null;
        }
    }

    /**
     * Resolves the client's InetSocketAddress from the QUIC connection.
     * The remote address is on the parent QuicChannel (the QUIC connection),
     * not the QuicStreamChannel (which is a virtual child channel).
     */
    private static java.net.InetSocketAddress resolveLocalAddress(ChannelHandlerContext ctx) {
        Channel channel = ctx.channel();
        if (channel.parent() instanceof QuicChannel quicChannel) {
            return (java.net.InetSocketAddress) quicChannel.parent().localAddress();
        }
        return (java.net.InetSocketAddress) channel.localAddress();
    }

    private InetSocketAddress resolveClientAddress(ChannelHandlerContext ctx) {
        Channel channel = ctx.channel();
        if (channel.parent() instanceof QuicChannel quicChannel) {
            // QuicChannel.remoteAddress() returns QuicConnectionAddress (QUIC connection ID),
            // not the peer's network address. Use remoteSocketAddress() which returns the
            // actual InetSocketAddress of the remote UDP endpoint from the datagram channel.
            return (InetSocketAddress) quicChannel.remoteSocketAddress();
        }
        return (InetSocketAddress) channel.remoteAddress();
    }

    /**
     * RFC 9114 Section 4.2: field values MUST NOT contain NUL (0x00), CR (0x0d), or LF (0x0a).
     * NUL/CRLF in pseudo-header values would enable header injection when translating
     * HTTP/3 to HTTP/1.1 for backend connections.
     *
     * @param value the value to check; may be {@code null}
     * @return {@code true} if the value contains NUL, CR, or LF
     */
    private static boolean containsProhibitedChars(CharSequence value) {
        if (value == null) {
            return false;
        }
        for (int i = 0, len = value.length(); i < len; i++) {
            char c = value.charAt(i);
            if (c == '\0' || c == '\r' || c == '\n') {
                return true;
            }
        }
        return false;
    }
}
