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
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcConstants;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcDeadlineHandler;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcDetector;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketOverH2Handler;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.DefaultHttp2ResetFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PingFrame;
import io.netty.handler.codec.http2.Http2ResetFrame;
import io.netty.handler.codec.http2.Http2SettingsAckFrame;
import io.netty.handler.codec.http2.Http2SettingsFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import static io.netty.handler.codec.http.HttpResponseStatus.BAD_REQUEST;
import static io.netty.handler.codec.http.HttpResponseStatus.METHOD_NOT_ALLOWED;
import static io.netty.handler.codec.http.HttpResponseStatus.SERVICE_UNAVAILABLE;

public final class Http2ServerInboundHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(Http2ServerInboundHandler.class);

    // ─── RFC 9113 Section 5.1: HTTP/2 Stream State Tracking ──────────────────
    // Lightweight stream state machine per RFC 9113 Section 5.1. Each stream
    // transitions through these states based on HEADERS and DATA frame flags
    // (END_STREAM, END_HEADERS) and RST_STREAM receipt. This tracker enables
    // the proxy to reject invalid operations such as DATA on a closed or
    // half-closed (remote) stream.
    //
    // Thread safety: Most access is from the frontend EventLoop thread (channelRead),
    // but onLocalEndStream() is package-visible and called from DownstreamHandler on the
    // backend EventLoop. ConcurrentHashMap is required for this cross-EventLoop access.

    /**
     * RFC 9113 Section 5.1: HTTP/2 stream states relevant to the server (receiver) side.
     * We track a simplified subset — the full state machine includes reserved states
     * for PUSH_PROMISE which this proxy does not use (server push is deprecated in RFC 9113).
     */
    enum H2StreamState {
        /** Stream is open for both sending and receiving frames. */
        OPEN,
        /** Remote side (client) has sent END_STREAM; no more DATA/HEADERS from client. */
        HALF_CLOSED_REMOTE,
        /** Local side (server/proxy) has sent END_STREAM; no more DATA/HEADERS from proxy. */
        HALF_CLOSED_LOCAL,
        /** Stream is fully closed (both sides sent END_STREAM, or RST_STREAM received). */
        CLOSED
    }

    /**
     * Per-stream state tracker. Maps frontend HTTP/2 stream IDs to their current state.
     * Entries are added when HEADERS (non-endStream) are received and removed when the
     * stream transitions to CLOSED (endStream DATA, RST_STREAM, or GOAWAY cleanup).
     *
     * <p>Auto-cleanup: entries are removed on CLOSED transition to prevent unbounded growth.
     * Additionally, {@link #cleanupResources()} clears the map on connection close.</p>
     */
    private final Map<Integer, H2StreamState> streamStates = new ConcurrentHashMap<>();

    /**
     * Transitions a stream to HALF_CLOSED_REMOTE state when the client sends END_STREAM
     * on a HEADERS or DATA frame. If the stream was already HALF_CLOSED_LOCAL, the stream
     * transitions to CLOSED.
     *
     * @param streamId the HTTP/2 stream ID
     */
    private void onRemoteEndStream(int streamId) {
        H2StreamState current = streamStates.get(streamId);
        if (current == H2StreamState.HALF_CLOSED_LOCAL) {
            // Both sides have sent END_STREAM -> stream is fully closed.
            streamStates.remove(streamId);
        } else {
            streamStates.put(streamId, H2StreamState.HALF_CLOSED_REMOTE);
        }
    }

    /**
     * Records that the local side (proxy) has sent END_STREAM to the client for this stream.
     * Called from DownstreamHandler or when the proxy sends a final response with endStream=true.
     * If the stream was already HALF_CLOSED_REMOTE, transitions to CLOSED.
     *
     * @param streamId the HTTP/2 stream ID
     */
    void onLocalEndStream(int streamId) {
        H2StreamState current = streamStates.get(streamId);
        if (current == H2StreamState.HALF_CLOSED_REMOTE) {
            streamStates.remove(streamId);
        } else if (current != null) {
            streamStates.put(streamId, H2StreamState.HALF_CLOSED_LOCAL);
        }
    }

    /**
     * Checks whether a stream is in a state that allows receiving DATA frames from the client.
     * Per RFC 9113 Section 5.1, DATA can only be received on OPEN or HALF_CLOSED_LOCAL streams.
     *
     * @param streamId the HTTP/2 stream ID
     * @return {@code true} if DATA can be received on this stream
     */
    private boolean canReceiveData(int streamId) {
        H2StreamState state = streamStates.get(streamId);
        // If the stream is not tracked (e.g., HEADERS had endStream=true, or already cleaned up),
        // we cannot receive data. OPEN and HALF_CLOSED_LOCAL allow receiving.
        return state == H2StreamState.OPEN || state == H2StreamState.HALF_CLOSED_LOCAL;
    }

    /**
     * Maximum gRPC deadline clamp: 300 seconds (5 minutes) to prevent unbounded scheduling.
     */
    private static final long MAX_DEADLINE_NANOS = TimeUnit.SECONDS.toNanos(300);

    /**
     * gRPC health check response: length-prefixed protobuf HealthCheckResponse with status SERVING.
     * Format: [compression flag (1 byte)] [message length (4 bytes big-endian)] [protobuf payload]
     * Protobuf payload: field 1, varint 1 (SERVING) = 0x08, 0x01
     */
    private static final byte[] GRPC_HEALTH_RESPONSE_BYTES = {0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x01};

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final UpstreamRetryHandler retryHandler;
    private final boolean isTLSConnection;

    /**
     * H2-04: Maximum request body size in bytes. Cached from HttpConfiguration
     * at construction time to avoid repeated lookups on the hot path.
     */
    private final long maxRequestBodySize;

    // ─── F-11: SETTINGS/PING flood protection ───────────────────────────────
    // RFC 9113 Section 10.5 recommends ENHANCE_YOUR_CALM (0xb) for connection-level abuse.
    // Excessive SETTINGS or PING frames are a well-known HTTP/2 DoS vector (CVE-2019-9512,
    // CVE-2019-9515). We track frame counts in a sliding window and GOAWAY the connection
    // when the threshold is exceeded. The window resets every RATE_LIMIT_WINDOW_NS.
    /**
     * Maximum number of SETTINGS + PING frames allowed per rate-limit window.
     */
    private static final int CONTROL_FRAME_RATE_LIMIT = 100;

    /**
     * Rate-limit window duration in nanoseconds (10 seconds).
     */
    private static final long RATE_LIMIT_WINDOW_NS = 10_000_000_000L;

    /**
     * Counter for SETTINGS and PING frames received in the current window.
     * Accessed only from the EventLoop thread (channelRead), so no synchronization needed.
     */
    private int controlFrameCount;

    /**
     * Timestamp ({@link System#nanoTime()} based) marking the start of the current
     * rate-limit window. Initialized lazily on first control frame.
     */
    private long controlFrameWindowStartNanos;

    /**
     * Set to {@code true} after we send GOAWAY for rate-limit or aggregate body violation.
     * Prevents sending multiple GOAWAYs and short-circuits all subsequent frame processing.
     */
    private boolean goAwaySent;

    // ─── F-13: Per-connection aggregate body size limit ─────────────────────
    // Prevents a single HTTP/2 connection from consuming unbounded memory by
    // streaming data across many concurrent streams. Without this limit, an
    // attacker can open maxConcurrentStreams streams each sending just under
    // the per-stream body limit, exhausting proxy memory.
    /**
     * Default maximum aggregate body bytes across all streams on a connection (256 MB).
     */
    private static final long DEFAULT_MAX_CONNECTION_BODY_SIZE = 256L * 1024 * 1024;

    /**
     * Configurable per-connection aggregate body size limit in bytes.
     */
    private final long maxConnectionBodySize;

    /**
     * Running total of body bytes received across all streams on this connection.
     * Accessed only from the EventLoop thread, so a plain long suffices.
     */
    private long connectionBodyBytes;

    /**
     * RES-DRAIN: Graceful shutdown drain timeout in milliseconds. Cached from
     * HttpConfiguration at construction time. When doClose() is called, a GOAWAY
     * frame is sent and the actual channel close is delayed by this duration to
     * allow in-flight streams to complete per RFC 9113 Section 6.8.
     */
    private final long gracefulShutdownDrainMs;

    private final ConnectionPool connectionPool;
    private final ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlineFutures = new ConcurrentHashMap<>();
    private ChannelHandlerContext ctx;
    /**
     * Maps frontend stream IDs to their backend HttpConnection. Populated when HEADERS
     * frames arrive (which carry :authority), then used to route subsequent DATA frames
     * (which do not carry :authority) to the correct backend connection.
     *
     * <p>F-01: ConcurrentHashMap is intentional — NOT single-threaded-only. The
     * {@link #registerBackendCloseListener} closure accesses this map from the backend
     * channel's closeFuture listener, which fires on the backend's EventLoop thread (may
     * differ from the frontend EventLoop thread since both use the shared childGroup).
     * Additionally, {@link #close()} is synchronized and callable from any thread.</p>
     */
    private final Map<Integer, HttpConnection> connectionsByStreamId = new ConcurrentHashMap<>();

    /**
     * H2-04: Per-stream accumulated request body byte counts. Used to enforce
     * the configured maximum request body size on HTTP/2 streams.
     *
     * <p>F-01: ConcurrentHashMap required for the same cross-thread access reasons
     * as {@link #connectionsByStreamId} — see its Javadoc.</p>
     */
    private final Map<Integer, Long> streamBodyBytes = new ConcurrentHashMap<>();

    /**
     * Active WebSocket-over-H2 handlers (RFC 8441).
     * Each Extended CONNECT stream gets its own handler.
     * HIGH-03: CopyOnWriteArrayList for thread-safety — add() happens on the EventLoop
     * thread in channelRead(), but close()/cleanupResources() may iterate from any thread.
     */
    private final List<WebSocketOverH2Handler> wsOverH2Handlers = new java.util.concurrent.CopyOnWriteArrayList<>();

    public Http2ServerInboundHandler(HTTPLoadBalancer httpLoadBalancer, boolean isTLSConnection) {
        this.httpLoadBalancer = httpLoadBalancer;
        bootstrapper = new Bootstrapper(httpLoadBalancer);
        retryHandler = new UpstreamRetryHandler(httpLoadBalancer, bootstrapper);
        this.isTLSConnection = isTLSConnection;
        this.connectionPool = new ConnectionPool(httpLoadBalancer.httpConfiguration());
        this.maxRequestBodySize = httpLoadBalancer.httpConfiguration().maxRequestBodySize();
        // F-13: Use configured value if available, otherwise fall back to default.
        // maxConnectionBodySize is read from HttpConfiguration if the field exists;
        // otherwise we use the 256 MB default.
        long configuredConnBodySize = httpLoadBalancer.httpConfiguration().maxConnectionBodySize();
        this.maxConnectionBodySize = configuredConnBodySize > 0 ? configuredConnBodySize : DEFAULT_MAX_CONNECTION_BODY_SIZE;
        this.gracefulShutdownDrainMs = httpLoadBalancer.httpConfiguration().gracefulShutdownDrainMs();
    }

    /**
     * F-16: Register a listener on the backend connection's channel close future
     * that reactively evicts the dead connection from the {@link ConnectionPool}
     * and {@link #connectionsByStreamId}. This prevents subsequent H2 streams from
     * being routed to a dead backend connection (black-holing).
     *
     * @param httpConnection the backend connection to monitor
     */
    private void registerBackendCloseListener(HttpConnection httpConnection) {
        // Use channelFuture().channel() which is available immediately after bootstrap.connect(),
        // rather than httpConnection.channel() which is set asynchronously after the connect
        // future completes (see Connection.init() listener).
        httpConnection.channelFuture().channel().closeFuture().addListener(future -> {
            // Evict from the connection pool.
            connectionPool.evict(httpConnection);

            // Evict all stream-ID mappings that pointed to this dead connection.
            // Iterator.remove() is safe on ConcurrentHashMap iterators.
            Iterator<Map.Entry<Integer, HttpConnection>> it = connectionsByStreamId.entrySet().iterator();
            while (it.hasNext()) {
                Map.Entry<Integer, HttpConnection> entry = it.next();
                if (entry.getValue() == httpConnection) {
                    streamBodyBytes.remove(entry.getKey());
                    it.remove();
                }
            }

            logger.debug("F-16: Evicted dead backend connection from pool");
        });
    }

    /**
     * F-16: Check whether a cached backend connection is still usable.
     *
     * <p>A connection is considered dead if its state has transitioned to
     * {@link com.shieldblaze.expressgateway.backend.Connection.State#CONNECTION_CLOSED}
     * or {@link com.shieldblaze.expressgateway.backend.Connection.State#CONNECTION_TIMEOUT},
     * or if the underlying Netty channel is no longer active. The state field is volatile,
     * so this check is safe from any thread.</p>
     *
     * @param conn the connection to check
     * @return {@code true} if the connection is dead and should be evicted
     */
    private static boolean isConnectionDead(HttpConnection conn) {
        com.shieldblaze.expressgateway.backend.Connection.State s = conn.state();
        if (s == com.shieldblaze.expressgateway.backend.Connection.State.CONNECTION_CLOSED
                || s == com.shieldblaze.expressgateway.backend.Connection.State.CONNECTION_TIMEOUT) {
            return true;
        }
        // Belt-and-suspenders: check the Netty channel directly in case the state
        // field hasn't been updated yet (race between closeFuture listener and this read).
        io.netty.channel.Channel ch = conn.channel();
        return ch != null && !ch.isActive();
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    /**
     * F-02: Aggregate backpressure for the multiplexed HTTP/2 frontend connection.
     *
     * <p>A single HTTP/2 frontend connection fans out to multiple backend connections
     * (one per :authority). {@link DownstreamHandler#channelWritabilityChanged} on each
     * backend channel individually toggles the frontend's {@code autoRead}, but this
     * creates a race: backend A becomes writable and re-enables {@code autoRead}, even
     * though backend B is still unwritable. The frontend then reads more frames destined
     * for backend B, which pile up in its outbound buffer.</p>
     *
     * <p>This override fires on the <em>frontend</em> channel when its own writability
     * changes (outbound buffer to the client crosses the high/low watermark). When the
     * frontend becomes unwritable, we pause reads on all backend channels so response
     * data stops flowing. When it becomes writable again, we resume reads on backends.</p>
     *
     * <p>For the request direction (frontend -> backend), we check aggregate backend
     * writability: {@code autoRead} on the frontend is only re-enabled when <em>all</em>
     * active backend channels are writable. This prevents one writable backend from
     * overriding the pause caused by an unwritable sibling.</p>
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        boolean frontendWritable = ctx.channel().isWritable();

        // GC-05: Use forEachActiveConnection to avoid ArrayList allocation per call.
        // Response direction: toggle autoRead on all backend channels based on
        // whether the frontend (client-facing) channel can accept more outbound data.
        connectionPool.forEachActiveConnection(conn -> {
            io.netty.channel.Channel backendCh = conn.channel();
            if (backendCh != null && backendCh.isActive()) {
                backendCh.config().setAutoRead(frontendWritable);
            }
        });

        // Request direction: only allow reads on the frontend if ALL backend channels
        // are writable. This prevents data from piling up in a slow backend's outbound
        // buffer when the frontend is reading faster than the backend can drain.
        ctx.channel().config().setAutoRead(connectionPool.allBackendsWritable());

        super.channelWritabilityChanged(ctx);
    }

    /**
     * GC-01: Flush coalescing for the HTTP/2 frontend-to-backend write path.
     *
     * <p>When Netty delivers a batch of inbound HTTP/2 frames (e.g., HEADERS followed
     * by multiple DATA frames across several streams), each {@code channelRead()} call
     * writes to the appropriate backend connection using {@code channel.write()} (no
     * flush) for intermediate DATA frames. After the entire batch is delivered, Netty
     * calls {@code channelReadComplete()}, at which point we flush all backend
     * connections that may have buffered writes.</p>
     *
     * <p>This mirrors the HIGH-10 flush coalescing pattern used in the TCP L4 proxy.
     * For a multiplexed HTTP/2 connection carrying N concurrent streams, this reduces
     * backend syscalls from one-per-DATA-frame to one-per-backend-connection-per-batch.</p>
     *
     * <p>We use {@code forEachActiveConnection} to avoid allocating a temporary
     * collection on every read-complete callback. The flush is a no-op on connections
     * that have no buffered writes (Netty's flush is idempotent when the outbound
     * buffer is empty).</p>
     */
    @Override
    public void channelReadComplete(ChannelHandlerContext ctx) throws Exception {
        connectionPool.forEachActiveConnection(HttpConnection::flush);
        super.channelReadComplete(ctx);
    }

    /**
     * F-11: Check whether the SETTINGS/PING control frame rate limit has been exceeded.
     * Uses a simple sliding-window approach: counts frames within a fixed window, resets
     * the window when it expires. If the limit is exceeded, sends GOAWAY with
     * ENHANCE_YOUR_CALM and returns {@code true}.
     *
     * <p>Must be called only from the EventLoop thread (channelRead).</p>
     *
     * @param ctx the channel handler context for writing the GOAWAY
     * @return {@code true} if the rate limit was exceeded and GOAWAY was sent
     */
    private boolean checkControlFrameRateLimit(ChannelHandlerContext ctx) {
        long now = System.nanoTime();
        long elapsed = now - controlFrameWindowStartNanos;
        if (elapsed > RATE_LIMIT_WINDOW_NS) {
            // Window expired. Instead of resetting to 0 (which allows a burst of
            // CONTROL_FRAME_RATE_LIMIT frames right after the boundary), carry over
            // a proportional fraction of the count from the previous window. This
            // approximates a sliding window and prevents boundary-crossing bursts.
            //
            // Example: if 80 frames were counted and the window just expired,
            // carry over 80 * (overlap / window). If elapsed >> window, the
            // carry-over is effectively 0, which is correct for long idle periods.
            double overlap = Math.max(0, 1.0 - (double) elapsed / RATE_LIMIT_WINDOW_NS);
            controlFrameCount = (int) (controlFrameCount * overlap);
            controlFrameWindowStartNanos = now;
        }
        controlFrameCount++;
        if (controlFrameCount > CONTROL_FRAME_RATE_LIMIT) {
            if (!goAwaySent) {
                goAwaySent = true;
                logger.warn("F-11: SETTINGS/PING flood detected ({} frames in window), sending GOAWAY ENHANCE_YOUR_CALM",
                        controlFrameCount);
                ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.ENHANCE_YOUR_CALM))
                        .addListener(ChannelFutureListener.CLOSE);
            }
            return true;
        }
        return false;
    }

    /**
     * SEC-01: Check whether a {@link CharSequence} contains CR ({@code 0x0D}) or LF
     * ({@code 0x0A}) characters. CRLF in HTTP/2 pseudo-header values is a request
     * smuggling / header injection vector when the proxy converts H2 to H1: the
     * {@code :path} value becomes the HTTP/1.1 request-line, and CRLF characters
     * would inject additional headers or a second request into the backend connection.
     *
     * <p>RFC 9113 Section 8.2.1 states that field values "MUST NOT include" CR, LF,
     * or NUL characters. While the HPACK decoder may reject some of these, the
     * check is defense-in-depth: we validate at the application layer to catch any
     * values that survive the codec, regardless of HPACK encoding tricks.</p>
     *
     * <p>This method performs a linear scan (O(n) in the length of the value).
     * For typical pseudo-header values (:path, :authority, :scheme), n is small
     * (generally &lt; 8192 bytes, bounded by SETTINGS_MAX_HEADER_LIST_SIZE).</p>
     *
     * @param value the header value to check; may be {@code null}
     * @return {@code true} if the value contains {@code \r} or {@code \n}
     */
    static boolean containsCRLF(CharSequence value) {
        if (value == null) {
            return false;
        }
        for (int i = 0, len = value.length(); i < len; i++) {
            char c = value.charAt(i);
            if (c == '\r' || c == '\n') {
                return true;
            }
        }
        return false;
    }

    /**
     * H2-REQ-01 / H2-REQ-02 / H2-REQ-03 / P2-6 / P2-7 / SEC-01 / SEC-07: Validate HTTP/2
     * pseudo-headers on inbound request HEADERS frames per RFC 9113 Section 8.3.
     *
     * <p>RFC 9113 Section 8.3 defines the following rules for request pseudo-headers:</p>
     * <ul>
     *   <li><b>H2-REQ-01</b> (Section 8.3.1): All HTTP/2 requests MUST include exactly one
     *       value for the {@code :method}, {@code :scheme}, and {@code :path} pseudo-header
     *       fields, unless the request is a CONNECT request (Section 8.5).</li>
     *   <li><b>P2-6</b> (Section 8.3.1): For non-CONNECT requests targeting http/https URIs,
     *       the {@code :authority} pseudo-header MUST be present. Missing {@code :authority}
     *       is rejected with 400 Bad Request.</li>
     *   <li><b>P2-7</b> (Section 8.3.1): The {@code :path} pseudo-header MUST start with
     *       {@code "/"} for non-CONNECT requests, except {@code OPTIONS *} which may use
     *       asterisk-form. Invalid format is rejected with 400 Bad Request.</li>
     *   <li><b>H2-REQ-02</b> (Section 8.3): A request MUST NOT contain response pseudo-headers
     *       ({@code :status}). The presence of {@code :status} in a request is a malformed
     *       message.</li>
     *   <li><b>H2-REQ-03</b> (Section 8.3): All pseudo-header fields MUST appear in the header
     *       block before regular header fields. A request that contains a pseudo-header field
     *       after a regular header field is malformed.</li>
     *   <li><b>SEC-01</b> (CRLF injection): The {@code :path}, {@code :authority}, and
     *       {@code :scheme} pseudo-header values MUST NOT contain CR or LF characters.
     *       Per RFC 9113 Section 8.2.1, field values "MUST NOT include" CR, LF, or NUL.
     *       CRLF in these values is a header injection / request smuggling vector when
     *       the proxy converts H2 to H1 (the :path becomes the request-line, :authority
     *       becomes the Host header, and :scheme is used in Forwarded/X-Forwarded-Proto).
     *       Violations are rejected with RST_STREAM(PROTOCOL_ERROR) because they represent
     *       a malformed message at the HTTP/2 framing level.</li>
     *   <li><b>SEC-07 / RS-09</b> (Section 8.3.1): If both {@code :authority} and
     *       {@code Host} are present, they MUST have the same value (case-insensitive).
     *       Differing values cause a routing desync: the proxy routes on {@code :authority}
     *       while the H2-to-H1 conversion may emit the original {@code Host} to the backend,
     *       allowing a request targeting one virtual host at the proxy to reach a different
     *       virtual host on the backend. Rejected with 400 Bad Request.</li>
     * </ul>
     *
     * <p>For framing-level violations (H2-REQ-01/02/03/SEC-01), this method sends
     * {@code RST_STREAM} with {@link Http2Error#PROTOCOL_ERROR}. For semantic violations
     * (P2-6/P2-7), it sends a 400 Bad Request response. In both cases, the message is
     * released and {@code false} is returned. The caller MUST return immediately when
     * {@code false} is returned.</p>
     *
     * <p>Must be called only from the EventLoop thread (channelRead).</p>
     *
     * @param headersFrame the inbound HTTP/2 HEADERS frame to validate
     * @param ctx          the channel handler context for writing RST_STREAM
     * @return {@code true} if validation passes and processing should continue;
     *         {@code false} if a protocol error was detected and RST_STREAM was sent
     */
    private boolean validatePseudoHeaders(Http2HeadersFrame headersFrame, ChannelHandlerContext ctx) {
        Http2Headers headers = headersFrame.headers();

        // ── H2-REQ-02: Reject response pseudo-headers in a request ──────────────
        // RFC 9113 Section 8.3: "Pseudo-header fields defined for requests MUST NOT
        // appear in responses; pseudo-header fields defined for responses MUST NOT
        // appear in requests."
        if (headers.status() != null) {
            logger.warn("H2-REQ-02: Request contains forbidden :status pseudo-header on stream {}",
                    headersFrame.stream().id());
            sendRstStreamProtocolError(headersFrame, ctx);
            return false;
        }

        // ── H2-REQ-01: Validate required pseudo-headers ─────────────────────────
        // RFC 9113 Section 8.3.1: "All HTTP/2 requests MUST include exactly one valid
        // value for the ':method', ':scheme', and ':path' pseudo-header fields, unless
        // the request is a CONNECT request (Section 8.5)."
        //
        // CONNECT requests only require :method and :authority (Section 8.5); we do
        // not support CONNECT (it would be rejected later), but we still validate
        // correctly to avoid false PROTOCOL_ERROR on well-formed CONNECT requests.
        CharSequence method = headers.method();
        if (method == null) {
            logger.warn("H2-REQ-01: Missing required :method pseudo-header on stream {}",
                    headersFrame.stream().id());
            sendRstStreamProtocolError(headersFrame, ctx);
            return false;
        }

        boolean isConnect = "CONNECT".contentEquals(method);

        // GAP-H2-02: RFC 9113 Section 8.5 — A CONNECT request MUST NOT include
        // :scheme or :path pseudo-headers. Their presence indicates a malformed
        // CONNECT request (distinct from Extended CONNECT per RFC 8441 which uses
        // :protocol and IS allowed to include :scheme and :path).
        if (isConnect && (headers.scheme() != null || headers.path() != null)) {
            // Only reject plain CONNECT, not Extended CONNECT (which has :protocol).
            // Extended CONNECT is handled separately in channelRead() with :protocol check.
            if (headers.get(":protocol") == null) {
                logger.warn("H2-CONNECT-01: CONNECT request includes :scheme or :path on stream {}, rejecting",
                        headersFrame.stream().id());
                sendRstStreamProtocolError(headersFrame, ctx);
                return false;
            }
        }

        if (!isConnect) {
            if (headers.scheme() == null) {
                logger.warn("H2-REQ-01: Missing required :scheme pseudo-header on stream {}",
                        headersFrame.stream().id());
                sendRstStreamProtocolError(headersFrame, ctx);
                return false;
            }
            if (headers.path() == null) {
                logger.warn("H2-REQ-01: Missing required :path pseudo-header on stream {}",
                        headersFrame.stream().id());
                sendRstStreamProtocolError(headersFrame, ctx);
                return false;
            }
        }

        // ── SEC-01: CRLF injection defense ───────────────────────────────────────
        // RFC 9113 Section 8.2.1: "A field value MUST NOT contain the zero value
        // (ASCII NUL, 0x00), line feed (ASCII LF, 0x0a), or carriage return
        // (ASCII CR, 0x0d) at any position."
        //
        // When this proxy converts H2 to H1 for backend connections:
        //   - :path   -> HTTP/1.1 request-line (e.g., "GET /path HTTP/1.1\r\n")
        //   - :authority -> Host header
        //   - :scheme -> X-Forwarded-Proto / Forwarded header values
        //
        // CRLF in any of these values would inject additional headers or a second
        // request into the backend H1 connection — a classic HTTP desync/smuggling
        // attack. We validate all three pseudo-headers that participate in H1
        // conversion. :method is constrained to a token (no whitespace by definition)
        // and is validated by Netty's codec, so it is not checked here.
        //
        // This check uses RST_STREAM(PROTOCOL_ERROR) because CRLF in a pseudo-header
        // value is a framing-level violation (malformed message per RFC 9113 Section 8.2.1),
        // not a semantic error.
        if (containsCRLF(headers.path())) {
            logger.warn("SEC-01: CRLF injection attempt in :path pseudo-header on stream {}",
                    headersFrame.stream().id());
            sendRstStreamProtocolError(headersFrame, ctx);
            return false;
        }
        if (containsCRLF(headers.authority())) {
            logger.warn("SEC-01: CRLF injection attempt in :authority pseudo-header on stream {}",
                    headersFrame.stream().id());
            sendRstStreamProtocolError(headersFrame, ctx);
            return false;
        }
        if (containsCRLF(headers.scheme())) {
            logger.warn("SEC-01: CRLF injection attempt in :scheme pseudo-header on stream {}",
                    headersFrame.stream().id());
            sendRstStreamProtocolError(headersFrame, ctx);
            return false;
        }

        // P2-6: RFC 9113 Section 8.3.1 — For non-CONNECT requests, :authority MUST be
        // present. While the RFC uses "SHOULD", Section 8.3.1 also states that "If the
        // :scheme pseudo-header field identifies a scheme that has a mandatory authority
        // component (including 'http' and 'https'), the request MUST contain either an
        // :authority pseudo-header field or a Host header field." Since this proxy only
        // handles http/https, we enforce :authority as mandatory and reject with 400.
        // CONNECT requests use :authority differently (Section 8.5) and are validated
        // separately.
        if (!isConnect && headers.authority() == null) {
            logger.warn("P2-6: Missing :authority pseudo-header on non-CONNECT request, stream {} — " +
                    "rejecting with 400 per RFC 9113 Section 8.3.1", headersFrame.stream().id());
            send400BadRequest(headersFrame, ctx);
            return false;
        }

        // ── SEC-07 / RS-09: :authority / Host consistency ─────────────────────────
        // RFC 9113 Section 8.3.1: "If the :scheme pseudo-header field identifies a
        // scheme that has a mandatory authority component (including 'http' and
        // 'https'), the request MUST contain either an :authority pseudo-header field
        // or a Host header field. If a Host header field is present, the request MUST
        // contain an :authority pseudo-header field with the same value as the Host
        // header."
        //
        // When both :authority and Host are present with different values, the proxy
        // would route on :authority but the H2-to-H1 conversion emits Host from the
        // original header — a request desync vector where the backend virtual-host
        // selection differs from the proxy's routing decision.
        CharSequence host = headers.get("host");
        if (!isConnect && headers.authority() != null && host != null) {
            if (!headers.authority().toString().equalsIgnoreCase(host.toString())) {
                logger.warn("SEC-07: :authority '{}' and Host '{}' differ on stream {}, rejecting with 400",
                        sanitize(headers.authority()), sanitize(host), headersFrame.stream().id());
                send400BadRequest(headersFrame, ctx);
                return false;
            }
        }

        // P2-7: RFC 9113 Section 8.3.1 — The :path pseudo-header field includes the
        // path and query components of the target URI. It MUST NOT be empty for
        // "http" or "https" URIs; it MUST start with "/" (absolute-path) except for
        // an OPTIONS request where the value can be "*" (asterisk-form, RFC 9110
        // Section 7.1). Missing :path was already rejected above; here we validate
        // the format of the :path value.
        if (!isConnect) {
            CharSequence path = headers.path();
            // path != null is guaranteed here (missing :path was rejected above)
            if (path.length() == 0) {
                logger.warn("P2-7: Empty :path pseudo-header on stream {} — rejecting with 400 " +
                        "per RFC 9113 Section 8.3.1", headersFrame.stream().id());
                send400BadRequest(headersFrame, ctx);
                return false;
            }

            boolean isOptions = "OPTIONS".contentEquals(method);
            boolean isAsteriskForm = path.length() == 1 && path.charAt(0) == '*';

            // OPTIONS requests MAY use asterisk-form ("*") per RFC 9110 Section 7.1.
            // All other requests MUST use absolute-path form starting with "/".
            if (!(isOptions && isAsteriskForm) && path.charAt(0) != '/') {
                logger.warn("P2-7: Invalid :path '{}' on stream {} — must start with '/' " +
                        "(or '*' for OPTIONS). Rejecting with 400 per RFC 9113 Section 8.3.1",
                        sanitize(path), headersFrame.stream().id());
                send400BadRequest(headersFrame, ctx);
                return false;
            }
        }

        // ── H2-REQ-03: Validate pseudo-header ordering ──────────────────────────
        // RFC 9113 Section 8.3: "All pseudo-header fields MUST appear in the header
        // block before regular header fields."
        //
        // We iterate the headers in wire order. Once we see a header name that does
        // NOT start with ':', any subsequent header starting with ':' is a violation.
        // The Http2Headers iteration order preserves insertion (wire) order.
        boolean regularHeaderSeen = false;
        for (Map.Entry<CharSequence, CharSequence> entry : headers) {
            CharSequence name = entry.getKey();
            if (name.length() > 0 && name.charAt(0) == ':') {
                if (regularHeaderSeen) {
                    logger.warn("H2-REQ-03: Pseudo-header '{}' appears after regular headers on stream {}",
                            sanitize(name), headersFrame.stream().id());
                    sendRstStreamProtocolError(headersFrame, ctx);
                    return false;
                }
            } else {
                regularHeaderSeen = true;
            }
        }

        return true;
    }

    /**
     * Sends {@code RST_STREAM} with {@link Http2Error#PROTOCOL_ERROR} on the stream associated
     * with the given HEADERS frame and releases the frame's reference count.
     *
     * @param headersFrame the offending HEADERS frame (released after RST_STREAM is written)
     * @param ctx          the channel handler context for writing RST_STREAM
     */
    private static void sendRstStreamProtocolError(Http2HeadersFrame headersFrame, ChannelHandlerContext ctx) {
        // CRIT-01/02: Save the stream reference BEFORE releasing the frame, because
        // release may invalidate the frame's internal state (use-after-release).
        Http2FrameStream stream = headersFrame.stream();
        ReferenceCountUtil.release(headersFrame);
        DefaultHttp2ResetFrame rstFrame = new DefaultHttp2ResetFrame(Http2Error.PROTOCOL_ERROR);
        rstFrame.stream(stream);
        ctx.writeAndFlush(rstFrame);
    }

    /**
     * P2-6/P2-7: Sends an HTTP/2 400 Bad Request response on the stream associated with the
     * given HEADERS frame and releases the frame's reference count. Used for semantic validation
     * failures (missing :authority, invalid :path format) where the request is well-formed at
     * the HTTP/2 framing level but violates RFC 9113 Section 8.3.1 request semantics.
     *
     * <p>A 400 response (rather than RST_STREAM PROTOCOL_ERROR) is appropriate here because
     * the issue is at the HTTP semantics layer, not the framing layer. The client sent a
     * syntactically valid HEADERS frame but with invalid pseudo-header values. Per RFC 9113
     * Section 8.1.1, an endpoint that detects a malformed request "MAY" respond with a
     * stream error OR a 4xx status code. We prefer 400 for better client diagnostics.</p>
     *
     * @param headersFrame the offending HEADERS frame (released after the response is written)
     * @param ctx          the channel handler context for writing the 400 response
     */
    private static void send400BadRequest(Http2HeadersFrame headersFrame, ChannelHandlerContext ctx) {
        // CRIT-01/02: Save the stream reference BEFORE releasing the frame, because
        // release may invalidate the frame's internal state (use-after-release).
        Http2FrameStream stream = headersFrame.stream();
        ReferenceCountUtil.release(headersFrame);
        Http2Headers responseHeaders = new DefaultHttp2Headers();
        responseHeaders.status(BAD_REQUEST.codeAsText());
        responseHeaders.set("date", httpDate());
        Http2HeadersFrame responseFrame = new DefaultHttp2HeadersFrame(responseHeaders, true);
        responseFrame.stream(stream);
        ctx.writeAndFlush(responseFrame);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        // F-11: If we already sent GOAWAY for abuse, short-circuit all frame processing.
        // The connection is draining; no new work should be accepted.
        if (goAwaySent) {
            ReferenceCountUtil.release(msg);
            return;
        }

        // H2-03: RFC 9113 Section 6.5 — SETTINGS frames are connection-level and MUST NOT
        // be forwarded between independent connections. Each HTTP/2 connection negotiates
        // its own settings. Release to prevent reference count leaks.
        //
        // F-11: Also rate-limit SETTINGS frames. Excessive SETTINGS is a known DoS vector
        // (CVE-2019-9515 "SETTINGS Flood"). checkControlFrameRateLimit() sends GOAWAY
        // ENHANCE_YOUR_CALM if the threshold is exceeded.
        if (msg instanceof Http2SettingsFrame || msg instanceof Http2SettingsAckFrame) {
            checkControlFrameRateLimit(ctx);
            ReferenceCountUtil.release(msg);
            return;
        }

        // F-11: Rate-limit PING frames. Excessive PING is a known DoS vector
        // (CVE-2019-9512 "Ping Flood"). Note: autoAckPingFrame is true in
        // CompressibleHttp2FrameCodec, so Netty's codec has already sent the PING ACK.
        // We only need to count and enforce the rate limit here.
        //
        // GRPC-KA: gRPC keepalive PINGs are safe under this rate limit. The gRPC spec
        // default keepalive interval is 120 seconds (GRPC_ARG_KEEPALIVE_TIME_MS), with
        // a 20-second timeout PING. Even aggressive clients using 10-second intervals
        // produce only 1 PING per 10s — well below CONTROL_FRAME_RATE_LIMIT (100/10s).
        // The gRPC KEEPALIVE_PERMIT_WITHOUT_CALLS setting does not change PING frequency
        // beyond what the interval dictates. Legitimate gRPC keepalive will never trigger
        // this rate limit; only abusive PING floods will.
        if (msg instanceof Http2PingFrame) {
            checkControlFrameRateLimit(ctx);
            ReferenceCountUtil.release(msg);
            return;
        }

        // H2-01: RFC 9113 Section 6.4 — RST_STREAM from client signals stream cancellation.
        // Propagate to the backend so it can stop processing the request.
        if (msg instanceof Http2ResetFrame resetFrame) {
            int streamId = resetFrame.stream().id();
            HttpConnection conn = connectionsByStreamId.remove(streamId);
            streamBodyBytes.remove(streamId);
            // RFC 9113 Section 5.1: RST_STREAM transitions the stream to CLOSED.
            streamStates.remove(streamId);
            // GRPC-DL-01: Cancel the gRPC deadline future for this stream. Without this,
            // the ScheduledFuture leaks in the deadlineFutures map until it fires, wasting
            // memory and EventLoop scheduling overhead for already-cancelled streams.
            GrpcDeadlineHandler.cancelDeadline(deadlineFutures, streamId);
            if (conn != null) {
                // Stream cancelled — decrement active stream count and notify pool.
                // OBS-03: The metric gauge is updated inside decrementActiveStreams().
                conn.decrementActiveStreams();
                if (conn.isHttp2() && conn.node() != null) {
                    connectionPool.releaseH2Stream(conn.node(), conn);
                }
                conn.writeAndFlush(resetFrame);
            } else {
                ReferenceCountUtil.release(msg);
            }
            return;
        }

        // H2-02: RFC 9113 Section 6.8 — GOAWAY from client signals it will not send new
        // streams. Forward to all backend connections so they can begin draining.
        if (msg instanceof Http2GoAwayFrame goAwayFrame) {
            int lastStreamId = goAwayFrame.lastStreamId();

            // F-03: RFC 9113 Section 6.8 — Streams with identifiers greater than lastStreamId
            // will never be processed by the client. Clean up connectionsByStreamId and
            // streamBodyBytes entries for those abandoned streams. Without this cleanup, map
            // entries for streams > lastStreamId leak until the entire connection closes.
            //
            // Collect keys first to avoid ConcurrentModificationException; although
            // ConcurrentHashMap iterators are weakly consistent and safe to use with
            // Iterator.remove(), collecting the affected stream IDs first is clearer
            // and lets us log them as a batch.
            List<Integer> staleStreamIds = new ArrayList<>();
            for (Integer streamId : connectionsByStreamId.keySet()) {
                if (streamId > lastStreamId) {
                    staleStreamIds.add(streamId);
                }
            }
            for (Integer streamId : staleStreamIds) {
                connectionsByStreamId.remove(streamId);
                streamBodyBytes.remove(streamId);
                streamStates.remove(streamId);
                // GRPC-DL-02: Cancel gRPC deadline futures for streams pruned by GOAWAY.
                GrpcDeadlineHandler.cancelDeadline(deadlineFutures, streamId);
            }
            if (!staleStreamIds.isEmpty()) {
                logger.debug("F-03: GOAWAY lastStreamId={}, cleaned up {} stale stream entries: {}",
                        lastStreamId, staleStreamIds.size(), staleStreamIds);
            }

            java.util.Collection<HttpConnection> h2Conns = connectionPool.allActiveH2Connections();
            if (h2Conns.isEmpty()) {
                ReferenceCountUtil.release(msg);
            } else {
                // F-17: Forward the debug data from the client's GOAWAY to each backend.
                // RFC 9113 Section 6.8: "The GOAWAY frame also contains a 32-bit error code
                // [...] followed by additional debug data." This opaque data aids diagnostics
                // and MUST be forwarded. Each backend gets a retainedDuplicate() since
                // multiple backends each need an independent refcount on the ByteBuf.
                for (HttpConnection conn : h2Conns) {
                    Http2GoAwayFrame forwarded = new DefaultHttp2GoAwayFrame(
                            goAwayFrame.errorCode(), goAwayFrame.content().retainedDuplicate());
                    conn.writeAndFlush(forwarded);
                }
                ReferenceCountUtil.release(msg);
            }

            // BUG-H2GA: RFC 9113 Section 6.8 — After receiving a GOAWAY from the client,
            // the proxy should close the frontend connection. The client has signaled it will
            // not initiate new streams. Without this close, the connection hangs indefinitely
            // and the client never sees the connection close event.
            //
            // BUG-H2GA-DRAIN: We must NOT use the normal close() -> doClose() path here.
            // doClose() sends GOAWAY through the Http2FrameCodec (which sets the codec's
            // internal needsGracefulShutdown flag), then schedules a delayed channel.close()
            // after gracefulShutdownDrainMs. When that delayed close fires, it hits the
            // Http2ConnectionHandler.close() method which — because needsGracefulShutdown is
            // now true — enters its OWN graceful shutdown with a 30-second default timeout
            // (Http2CodecUtil.DEFAULT_GRACEFUL_SHUTDOWN_TIMEOUT_MILLIS). Total delay:
            // gracefulShutdownDrainMs + 30s, far exceeding the client's expectations.
            //
            // Since the client initiated the close (it sent GOAWAY), no drain period is
            // needed. Close the channel directly without writing GOAWAY through the codec
            // (avoids triggering needsGracefulShutdown). The client already knows the
            // connection is closing — it sent GOAWAY itself.
            goAwaySent = true;
            final ChannelHandlerContext localCtx = ctx;
            ctx = null;
            localCtx.channel().close();
            cleanupResources();
            return;
        }

        if (msg instanceof Http2HeadersFrame headersFrame) {
            // H2-REQ-01 / H2-REQ-02 / H2-REQ-03 / SEC-01: Validate pseudo-headers per RFC 9113
            // Section 8.3 and check for CRLF injection (SEC-01). This MUST happen before any
            // routing logic so that malformed requests never reach the backend. On failure,
            // validatePseudoHeaders() sends RST_STREAM(PROTOCOL_ERROR) on the offending stream
            // and releases the message.
            if (!validatePseudoHeaders(headersFrame, ctx)) {
                return;
            }

            // ME-05: Health check endpoints — respond directly on the H2 stream
            CharSequence path = headersFrame.headers().path();
            if (path != null) {
                boolean isHealth = "/health".contentEquals(path);
                boolean isReady = "/ready".contentEquals(path);
                if (isHealth || isReady) {
                    Http2FrameStream stream = headersFrame.stream();
                    ReferenceCountUtil.release(headersFrame);
                    boolean ok = isHealth ? !goAwaySent : (!goAwaySent && hasHealthyBackend());
                    Http2Headers responseHeaders = new DefaultHttp2Headers()
                            .status(ok ? "200" : "503")
                            .set("content-length", "0");
                    DefaultHttp2HeadersFrame responseFrame = new DefaultHttp2HeadersFrame(responseHeaders, true);
                    responseFrame.stream(stream);
                    ctx.writeAndFlush(responseFrame);
                    return;
                }
            }

            CharSequence method = headersFrame.headers().method();

            // RFC 8441: Detect Extended CONNECT with :protocol = websocket
            CharSequence protocol = headersFrame.headers().get(":protocol"); // RFC 8441 :protocol pseudo-header
            if ("CONNECT".contentEquals(method) && "websocket".contentEquals(protocol)) {
                // WebSocket-over-HTTP/2 (RFC 8441 Extended CONNECT)
                InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
                String wsAuthority = String.valueOf(headersFrame.headers().authority());
                String wsPath = String.valueOf(headersFrame.headers().path());

                WebSocketOverH2Handler wsHandler = new WebSocketOverH2Handler(
                        httpLoadBalancer, headersFrame.stream(), wsAuthority, wsPath, socketAddress
                );
                wsOverH2Handlers.add(wsHandler);
                ctx.pipeline().addLast(wsHandler);

                logger.debug("WebSocket over H2: Extended CONNECT received for authority={}, path={}, stream={}",
                        wsAuthority, wsPath, headersFrame.stream().id());
                return;
            }

            // HIGH-05: Reject CONNECT and TRACE methods in HTTP/2 path.
            // CONNECT has special semantics in H2 (RFC 9113 Sec 8.5) not implemented here
            // (Extended CONNECT for WebSocket is handled above).
            // TRACE must not contain a body and is a security risk to proxy.
            if (method != null) {
                String methodStr = method.toString();
                if (HttpMethod.CONNECT.name().equalsIgnoreCase(methodStr) || HttpMethod.TRACE.name().equalsIgnoreCase(methodStr)) {
                    Http2Headers rejectHeaders = new DefaultHttp2Headers();
                    rejectHeaders.status(METHOD_NOT_ALLOWED.codeAsText());
                    rejectHeaders.set("date", httpDate());

                    Http2HeadersFrame rejectFrame = new DefaultHttp2HeadersFrame(rejectHeaders, true);
                    rejectFrame.stream(headersFrame.stream());

                    // CRIT-03: Do not close connection for per-stream errors in H2.
                    ctx.writeAndFlush(rejectFrame);
                    ReferenceCountUtil.release(msg);
                    return;
                }
            }

            // SPEC-4: Configurable method restriction per HttpConfiguration.allowedMethods().
            if (method != null && !httpLoadBalancer.httpConfiguration().allowedMethods().contains(method.toString())) {
                Http2Headers rejectHeaders = new DefaultHttp2Headers();
                rejectHeaders.status(METHOD_NOT_ALLOWED.codeAsText());
                rejectHeaders.set("date", httpDate());

                Http2HeadersFrame rejectFrame = new DefaultHttp2HeadersFrame(rejectHeaders, true);
                rejectFrame.stream(headersFrame.stream());

                ctx.writeAndFlush(rejectFrame);
                ReferenceCountUtil.release(msg);
                return;
            }

            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            String authority = String.valueOf(headersFrame.headers().authority());
            // CRIT-04
            Cluster cluster;
            try { cluster = httpLoadBalancer.cluster(authority); }
            catch (Exception e) {
                logger.warn("DEF-H2-01: Exception looking up cluster for authority '{}': {}", authority, e.getMessage(), e);
                cluster = null;
            }

            // H2-10: If no Cluster was found for that Hostname, respond 503 Service Unavailable.
            // 503 is more appropriate than 502 because the proxy itself is operational but has
            // no backend configured for the requested :authority (analogous to Nginx's "no upstream").
            if (cluster == null) {
                Http2Headers http2Headers = new DefaultHttp2Headers();
                http2Headers.status(SERVICE_UNAVAILABLE.codeAsText());
                http2Headers.set("date", httpDate());

                Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                responseHeadersFrame.stream(headersFrame.stream());

                ctx.writeAndFlush(responseHeadersFrame); // CRIT-03
                return;
            }

            // Per-stream load balancing: select a backend node for EVERY HEADERS frame.
            // This matches Envoy/Nginx behavior where each H2 stream can be independently
            // routed to any backend, enabling true per-request load distribution.
            Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, headersFrame.headers())).node();

            // H2-10: 503 Service Unavailable when backend node has reached max connections.
            if (node.connectionFull()) {
                Http2Headers http2Headers = new DefaultHttp2Headers();
                http2Headers.status(SERVICE_UNAVAILABLE.codeAsText());
                http2Headers.set("date", httpDate());

                Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                responseHeadersFrame.stream(headersFrame.stream());

                ctx.writeAndFlush(responseHeadersFrame); // CRIT-03
                return;
            }

            // Try to acquire a pooled H2 connection with available stream capacity.
            // CM-D1: acquireH2() atomically claims a stream slot on the returned connection,
            // so we track whether it came from the pool to avoid double-incrementing.
            HttpConnection httpConnection = connectionPool.acquireH2(node);
            boolean fromPool = httpConnection != null;

            if (httpConnection == null && connectionPool.canCreateH2Connection(node)) {
                // No pooled connection with capacity — create a new one.
                UpstreamRetryHandler.RetryResult result = retryHandler.attemptWithRetryH2(
                        cluster, node, ctx.channel(), headersFrame.headers(), socketAddress,
                        connectionPool);

                if (result == null) {
                    // H2-10: All connection attempts failed — return 503 Service Unavailable.
                    logger.warn("All backend connection attempts failed for {} {}",
                            sanitize(headersFrame.headers().method()), sanitize(headersFrame.headers().path()));
                    Http2Headers http2Headers = new DefaultHttp2Headers();
                    http2Headers.status(SERVICE_UNAVAILABLE.codeAsText());
                    http2Headers.set("date", httpDate());

                    Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    responseHeadersFrame.stream(headersFrame.stream());

                    ctx.writeAndFlush(responseHeadersFrame); // CRIT-03
                    return;
                }

                httpConnection = result.connection();
                result.node().addConnection(httpConnection);
                // Register as H2: TLS+ALPN is configured so ALPN will resolve to h2.
                // We must pass true explicitly because ALPN hasn't completed yet at
                // this point — conn.isHttp2() would still return false.
                connectionPool.register(result.node(), httpConnection, true);
                registerBackendCloseListener(httpConnection);
            }

            if (httpConnection == null) {
                // All H2 connections at capacity and cannot create more — try H1 fallback
                // or reject. For simplicity, return 503 as no capacity is available.
                logger.warn("No H2 backend connection capacity for node {}", node.socketAddress());
                Http2Headers http2Headers = new DefaultHttp2Headers();
                http2Headers.status(SERVICE_UNAVAILABLE.codeAsText());
                http2Headers.set("date", httpDate());

                Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                responseHeadersFrame.stream(headersFrame.stream());

                ctx.writeAndFlush(responseHeadersFrame);
                return;
            }

            // Detect gRPC requests by content-type BEFORE allocating backend resources.
            // gRPC health check is handled locally — no backend connection needed.
            boolean isGrpc = GrpcDetector.isGrpc(headersFrame.headers());
            if (isGrpc && GrpcDetector.isGrpcHealthCheck(headersFrame.headers())) {
                sendGrpcHealthResponse(ctx, headersFrame.stream());
                return;
            }

            // Track this stream on the backend connection for capacity management.
            // OBS-03: The metric gauge is updated inside incrementActiveStreams().
            // CM-D1: When acquireH2() returns a pooled connection, it already atomically
            // claimed a stream slot via tryIncrementActiveStreams(). Only newly created
            // connections (where httpConnection came from the create path above) need
            // an explicit increment here.
            if (!fromPool) {
                httpConnection.incrementActiveStreams();
            }

            // Map this stream ID to its backend connection so DATA frames can be routed correctly.
            // CF-02: Only store the mapping if more DATA frames will follow (endStream=false).
            if (!headersFrame.isEndStream()) {
                connectionsByStreamId.put(headersFrame.stream().id(), httpConnection);
                // RFC 9113 Section 5.1: Track stream state. The stream transitions to OPEN
                // when the client sends HEADERS without END_STREAM.
                streamStates.put(headersFrame.stream().id(), H2StreamState.OPEN);
            } else {
                // HEADERS with END_STREAM: stream transitions directly to HALF_CLOSED_REMOTE.
                // No need to track in streamStates — the stream has no further client-side
                // frames to validate, and the response will close it fully.
                onRemoteEndStream(headersFrame.stream().id());
            }

            // GAP-H2-01: RFC 9113 Section 8.2.2 — The only TE value permitted in HTTP/2
            // is "trailers". If TE contains any other value, the request is malformed.
            // This validation MUST happen before hop-by-hop stripping removes the TE header.
            CharSequence teValue = headersFrame.headers().get("te");
            if (teValue != null && !"trailers".contentEquals(teValue)) {
                logger.warn("H2-TE-01: TE header contains invalid value '{}' on stream {}, resetting stream",
                        sanitize(teValue), headersFrame.stream().id());
                // Clean up stream state since we are rejecting this request.
                connectionsByStreamId.remove(headersFrame.stream().id());
                streamBodyBytes.remove(headersFrame.stream().id());
                streamStates.remove(headersFrame.stream().id());
                httpConnection.decrementActiveStreams();
                if (httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }
                Http2FrameStream stream = headersFrame.stream();
                ReferenceCountUtil.release(headersFrame);
                ctx.writeAndFlush(new DefaultHttp2ResetFrame(Http2Error.PROTOCOL_ERROR).stream(stream));
                return;
            }

            // RFC 7230 Section 6.1: Strip hop-by-hop headers before forwarding to backend.
            // HTTP/2 should not carry these, but they may appear via protocol conversion.
            HopByHopHeaders.strip(headersFrame.headers());

            // Use set() instead of add() to prevent client-injected X-Forwarded-* header spoofing.
            // set() replaces any existing values sent by the client.
            headersFrame.headers().set(Headers.X_FORWARDED_FOR, socketAddress.getAddress().getHostAddress());
            headersFrame.headers().set(Headers.X_FORWARDED_PROTO, isTLSConnection ? "https" : "http");
            headersFrame.headers().set(Headers.X_FORWARDED_HOST, authority);
            headersFrame.headers().set(Headers.X_FORWARDED_PORT, String.valueOf(((InetSocketAddress) ctx.channel().localAddress()).getPort()));

            // HIGH-02: Forwarded header (RFC 7239)
            headersFrame.headers().set("forwarded", "for=" + socketAddress.getAddress().getHostAddress()
                    + ";proto=" + (isTLSConnection ? "https" : "http") + ";host=" + authority);
            // RFC 9110 Section 7.6.3: Via header
            headersFrame.headers().add(Headers.VIA, "2.0 expressgateway");

            // REQ-ID: Inject X-Request-ID for end-to-end request correlation.
            // If the client already provided one, preserve it — the client may be
            // chaining through multiple proxies or correlating with its own tracing
            // system. Only generate a new UUID v4 if absent.
            if (headersFrame.headers().get(Headers.X_REQUEST_ID) == null) {
                headersFrame.headers().set(Headers.X_REQUEST_ID, FastRequestId.generate());
            }

            // Schedule gRPC deadline if grpc-timeout header is present
            if (isGrpc) {
                CharSequence timeout = headersFrame.headers().get(GrpcConstants.GRPC_TIMEOUT);
                if (timeout != null) {
                    long parsedNanos = GrpcDeadlineHandler.parseTimeoutNanos(timeout);
                    if (parsedNanos > 0) {
                        final long deadlineNanos = Math.min(parsedNanos, MAX_DEADLINE_NANOS);
                        final int streamId = headersFrame.stream().id();
                        final Http2FrameStream clientStream = headersFrame.stream();
                        final ChannelHandlerContext capturedCtx = ctx;
                        // GRPC-DL-03: Capture the HttpConnection directly for the deadline
                        // lambda. connectionsByStreamId is cleared on request-body endStream,
                        // but the deadline must still fire if the response hasn't arrived.
                        final HttpConnection deadlineConn = httpConnection;
                        ScheduledFuture<?> deadline = ctx.executor().schedule(() -> {
                            // Remove from deadline map first to prevent double-fire.
                            if (deadlineFutures.remove(streamId) == null) {
                                // Already cancelled by a stream completion path.
                                return;
                            }
                            // Clean up the connection mapping if it still exists.
                            connectionsByStreamId.remove(streamId);
                            streamBodyBytes.remove(streamId);

                            // Send gRPC trailers-only response with :status 200 (required by
                            // RFC 9113) and grpc-status: 4 (DEADLINE_EXCEEDED) to the client.
                            Http2Headers trailers = new DefaultHttp2Headers()
                                    .status("200")
                                    .set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_DEADLINE_EXCEEDED)
                                    .set(GrpcConstants.GRPC_MESSAGE, "Deadline exceeded");
                            capturedCtx.writeAndFlush(
                                    new DefaultHttp2HeadersFrame(trailers, true).stream(clientStream));

                            // GRPC-DL-04: Send RST_STREAM with CANCEL (0x8) to the backend
                            // to stop processing. Per gRPC spec, a deadline exceeded is a
                            // cancellation from the backend's perspective.
                            // FIX: Set the client stream on the frame so writeIntoChannel()
                            // can look up the forward mapping and remap to the backend stream.
                            // Without this, stream() returns null causing NPE in writeIntoChannel.
                            DefaultHttp2ResetFrame rstFrame = new DefaultHttp2ResetFrame(Http2Error.CANCEL);
                            rstFrame.stream(clientStream);
                            deadlineConn.writeAndFlush(rstFrame);

                            // Clean up backend resources.
                            deadlineConn.decrementActiveStreams();
                            if (connectionPool != null && deadlineConn.node() != null) {
                                connectionPool.releaseH2Stream(deadlineConn.node(), deadlineConn);
                            }
                            logger.warn("gRPC deadline exceeded for stream {}", streamId);
                        }, deadlineNanos, TimeUnit.NANOSECONDS);
                        deadlineFutures.put(streamId, deadline);
                    }
                }
            }

            // GC-01: Forward HEADERS to backend via writeAndFlush. Although writeAndFlush
            // per frame is not ideal for pure coalescing, it is necessary because
            // Connection.writeAndFlush() handles protocol translation (H2->H1, H2->H2
            // stream remapping) and backlog queuing for connections still in INITIALIZED
            // state (ALPN pending). channelReadComplete() also calls flush on all active
            // backend connections as a belt-and-suspenders coalescing step.
            httpConnection.writeAndFlush(msg);
            return;
        }

        if (msg instanceof Http2DataFrame dataFrame) {
            int streamId = dataFrame.stream().id();
            int frameBytes = dataFrame.content().readableBytes();

            // RFC 9113 Section 5.1: Validate stream state before processing DATA.
            // DATA frames are only valid on OPEN or HALF_CLOSED_LOCAL streams.
            // Receiving DATA on a HALF_CLOSED_REMOTE or CLOSED stream is a stream error.
            if (!canReceiveData(streamId)) {
                logger.warn("RFC9113-5.1: DATA frame received on stream {} in invalid state (not OPEN/HALF_CLOSED_LOCAL), " +
                        "sending RST_STREAM STREAM_CLOSED", streamId);
                streamBodyBytes.remove(streamId);
                connectionsByStreamId.remove(streamId);
                ReferenceCountUtil.release(msg);
                DefaultHttp2ResetFrame resetFrame = new DefaultHttp2ResetFrame(Http2Error.STREAM_CLOSED);
                resetFrame.stream(dataFrame.stream());
                ctx.writeAndFlush(resetFrame);
                return;
            }

            // H2-04: Enforce per-stream request body size limit (RFC 9113 Section 8.1).
            // Track cumulative bytes per stream and send RST_STREAM with CANCEL
            // if the limit is exceeded, preventing resource exhaustion from oversized uploads.
            // CANCEL (0x8) is used instead of REFUSED_STREAM (0x7) because the proxy has
            // already received partial data — REFUSED_STREAM would incorrectly imply the
            // request was never processed and is safe to retry without side effects.
            long accumulated = streamBodyBytes.merge(streamId,
                    (long) frameBytes, Long::sum);
            if (accumulated > maxRequestBodySize) {
                streamBodyBytes.remove(streamId);
                connectionsByStreamId.remove(streamId);
                streamStates.remove(streamId);
                ReferenceCountUtil.release(msg);

                DefaultHttp2ResetFrame resetFrame = new DefaultHttp2ResetFrame(Http2Error.CANCEL);
                resetFrame.stream(dataFrame.stream());
                ctx.writeAndFlush(resetFrame);
                return;
            }

            // F-13: Enforce per-connection aggregate body size limit.
            // Even if each individual stream stays under the per-stream limit, the sum
            // across all concurrent streams can exhaust proxy memory. When exceeded, we
            // terminate the entire connection with GOAWAY ENHANCE_YOUR_CALM.
            // ENHANCE_YOUR_CALM (0xb) signals the peer is generating excessive load,
            // which is appropriate for aggregate body size abuse across multiplexed streams.
            connectionBodyBytes += frameBytes;
            if (connectionBodyBytes > maxConnectionBodySize) {
                if (!goAwaySent) {
                    goAwaySent = true;
                    logger.warn("F-13: Per-connection aggregate body limit exceeded ({} bytes > {} bytes), " +
                            "sending GOAWAY ENHANCE_YOUR_CALM", connectionBodyBytes, maxConnectionBodySize);
                    ReferenceCountUtil.release(msg);
                    ctx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.ENHANCE_YOUR_CALM))
                            .addListener(ChannelFutureListener.CLOSE);
                } else {
                    ReferenceCountUtil.release(msg);
                }
                return;
            }

            HttpConnection conn = connectionsByStreamId.get(streamId);
            if (conn != null) {
                // F-16: If the backend connection died between HEADERS and DATA frames,
                // the data would be silently dropped by writeAndFlush() (state == CONNECTION_CLOSED).
                // Instead, detect the dead connection, clean up, and send RST_STREAM to the client
                // so it knows the stream failed and can retry at the application level.
                if (isConnectionDead(conn)) {
                    connectionsByStreamId.remove(streamId);
                    streamBodyBytes.remove(streamId);
                    streamStates.remove(streamId);
                    // Decrement stream count for the dead connection.
                    conn.decrementActiveStreams();
                    ReferenceCountUtil.release(msg);

                    logger.debug("F-16: Backend connection died mid-stream for stream {}, sending RST_STREAM", streamId);
                    DefaultHttp2ResetFrame resetFrame = new DefaultHttp2ResetFrame(Http2Error.REFUSED_STREAM);
                    resetFrame.stream(dataFrame.stream());
                    ctx.writeAndFlush(resetFrame);
                    return;
                }

                conn.writeAndFlush(msg);
                // Clean up stream mapping when the request body ends.
                // Note: this is the REQUEST endStream — the response hasn't arrived yet.
                // Stream count management (decrementActiveStreams/pool release) happens in
                // DownstreamHandler when the RESPONSE endStream arrives from the backend.
                if (dataFrame.isEndStream()) {
                    connectionsByStreamId.remove(streamId);
                    streamBodyBytes.remove(streamId);
                    // RFC 9113 Section 5.1: Client sent END_STREAM on DATA -> transition
                    // to HALF_CLOSED_REMOTE (or CLOSED if we already sent END_STREAM).
                    onRemoteEndStream(streamId);
                }
            } else {
                ReferenceCountedUtil.silentRelease(msg);
            }
            return;
        }

        // Unrecognized frame type — release to prevent leaks
        ReferenceCountedUtil.silentRelease(msg);
    }

    /**
     * ME-03: Handle idle timeout by sending GOAWAY before closing the connection.
     * Per RFC 9113 Section 6.8, an endpoint SHOULD send GOAWAY before closing
     * a connection to allow the peer to know which streams were processed.
     * Without this, an idle H2 connection is abruptly closed (TCP RST),
     * which may cause the client to retry already-processed requests.
     */
    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        if (evt instanceof com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler.State) {
            // Idle timeout fired — initiate graceful close with GOAWAY
            close();
            return;
        }
        super.userEventTriggered(ctx, evt);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        close();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        try {
            if (cause instanceof IOException) {
                // H2-09: Log at debug level so operators can correlate connection resets
                // with client behavior without polluting error logs.
                logger.debug("IOException on connection", cause);
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            close();
        }
    }

    /**
     * Sends a gRPC health check response directly to the client without routing to the backend.
     */
    private void sendGrpcHealthResponse(ChannelHandlerContext ctx, Http2FrameStream stream) {
        // 1. Initial HEADERS
        Http2Headers responseHeaders = new DefaultHttp2Headers();
        responseHeaders.status("200");
        responseHeaders.set(GrpcConstants.CONTENT_TYPE, "application/grpc");
        ctx.write(new DefaultHttp2HeadersFrame(responseHeaders, false).stream(stream));

        // 2. DATA frame with protobuf HealthCheckResponse (SERVING)
        ctx.write(new DefaultHttp2DataFrame(Unpooled.wrappedBuffer(GRPC_HEALTH_RESPONSE_BYTES), false).stream(stream));

        // 3. Trailing HEADERS with grpc-status=0
        Http2Headers trailers = new DefaultHttp2Headers();
        trailers.set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_OK);
        ctx.writeAndFlush(new DefaultHttp2HeadersFrame(trailers, true).stream(stream));

        logger.debug("Responded to gRPC health check on stream {}", stream.id());
    }

    @Override
    public void close() {
        // Cancel all pending gRPC deadline futures on connection teardown.
        GrpcDeadlineHandler.cancelAllDeadlines(deadlineFutures);

        // HIGH-26: Schedule close() on EventLoop instead of using synchronized,
        // to avoid racing with channelRead() which always runs on the EventLoop thread.
        if (ctx != null && ctx.channel().eventLoop() != null && !ctx.channel().eventLoop().inEventLoop()) {
            ctx.channel().eventLoop().execute(this::doClose);
        } else {
            doClose();
        }
    }

    private void doClose() {
        if (ctx == null) {
            return;
        }

        // Capture ctx locally and null it out immediately to prevent re-entrant doClose()
        // from sending duplicate GOAWAYs or scheduling multiple delayed closes.
        final ChannelHandlerContext localCtx = ctx;
        ctx = null;

        // RES-DRAIN: Graceful shutdown per RFC 9113 Section 6.8.
        // Send GOAWAY to signal the client that no new streams will be accepted,
        // then delay the actual channel close to allow in-flight streams to complete.
        // This is the same pattern used by Nginx (lingering_close) and Envoy
        // (drain_timeout) to avoid cutting off active request/response exchanges.
        if (gracefulShutdownDrainMs > 0 && localCtx.channel().isActive() && !goAwaySent) {
            goAwaySent = true;
            logger.debug("RES-DRAIN: Sending GOAWAY and delaying close by {}ms for in-flight streams",
                    gracefulShutdownDrainMs);
            localCtx.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR));

            // Schedule the hard close after the drain timeout. The EventLoop's scheduled
            // task holds a reference to localCtx, which is fine — if the channel closes
            // before the timer fires (e.g., client sends GOAWAY back), close() is idempotent.
            localCtx.channel().eventLoop().schedule(() -> {
                if (localCtx.channel().isActive()) {
                    localCtx.channel().close();
                }
                cleanupResources();
            }, gracefulShutdownDrainMs, java.util.concurrent.TimeUnit.MILLISECONDS);
        } else {
            // No drain: close immediately (drain disabled, channel already inactive,
            // or GOAWAY was already sent for another reason like rate-limiting).
            localCtx.channel().close();
            cleanupResources();
        }
    }

    /**
     * Clean up all backend connections, stream mappings, and WebSocket handlers.
     * Extracted from doClose() so it can be called both in the immediate-close path
     * and in the deferred-close path after the drain timeout expires.
     */
    private void cleanupResources() {
        connectionPool.closeAll();
        connectionsByStreamId.clear();
        streamBodyBytes.clear();
        streamStates.clear();

        // Close all WebSocket-over-H2 handlers
        for (WebSocketOverH2Handler handler : wsOverH2Handlers) {
            handler.close();
        }
        wsOverH2Handlers.clear();
    }

    /**
     * GAP-H2-06: RFC 9110 Section 6.6.1 — A server MUST send a Date header field
     * in all responses (except 1xx and 5xx when the clock is unreliable). Proxy-generated
     * error responses were missing this header.
     *
     * @return the current date/time formatted per RFC 1123 (e.g., "Wed, 25 Mar 2026 12:00:00 GMT")
     */
    private static String httpDate() {
        return java.time.format.DateTimeFormatter.RFC_1123_DATE_TIME
                .format(java.time.ZonedDateTime.now(java.time.ZoneOffset.UTC));
    }

    /**
     * ME-05: Check if at least one backend node exists in any cluster.
     */
    private boolean hasHealthyBackend() {
        try {
            return httpLoadBalancer.clusters().values().stream()
                    .anyMatch(c -> !c.allNodes().isEmpty());
        } catch (Exception e) {
            return false;
        }
    }
}
