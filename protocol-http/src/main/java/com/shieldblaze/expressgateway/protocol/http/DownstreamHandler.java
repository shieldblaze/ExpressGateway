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

import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.compression.CompressionUtil;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcConstants;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcDetector;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PingFrame;
import io.netty.handler.codec.http2.Http2ResetFrame;
import io.netty.handler.codec.http2.Http2SettingsAckFrame;
import io.netty.handler.codec.http2.Http2SettingsFrame;
import io.netty.handler.codec.http2.Http2StreamFrame;
import io.netty.handler.codec.http2.Http2WindowUpdateFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.timeout.ReadTimeoutException;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.util.concurrent.ConcurrentHashMap;

import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpResponseStatus.GATEWAY_TIMEOUT;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;
import static io.netty.handler.codec.http2.HttpConversionUtil.toHttp2Headers;

final class DownstreamHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final HttpConnection httpConnection;
    private final boolean isConnectionHttp2;
    /**
     * Connection pool for returning idle connections after response completion.
     * May be {@code null} for legacy code paths that do not use pooling.
     */
    private final ConnectionPool connectionPool;
    // HIGH-04: volatile for safe cross-thread visibility — set on EventLoop,
    // read/nulled in synchronized close() which may be called from any thread.
    private volatile ChannelHandlerContext ctx;
    private volatile Channel inboundChannel;
    private boolean headerRead;

    /**
     * DEF-H2-02: Maximum response body size in bytes. 0 = unlimited.
     */
    private final long maxResponseBodySize;

    /**
     * DEF-H2-02: Running total of response body bytes received from this backend
     * connection (H1 path). Reset is not needed because each DownstreamHandler is
     * bound to exactly one backend connection lifecycle.
     */
    private long responseBodyBytes;

    /**
     * Tracks backend stream IDs that are part of gRPC exchanges.
     * Used to detect missing grpc-status trailers when the backend disconnects.
     */
    private final java.util.Set<Integer> activeGrpcStreamIds = ConcurrentHashMap.newKeySet();
    /**
     * H1-07: Tracks whether the backend sent "Connection: close", indicating
     * the backend will close its end after the current response. We must close
     * the backend channel once the full response has been forwarded.
     */
    private boolean backendConnectionClose;

    /**
     * GRPC-TV-02: Tracks whether the current H1->H2 translated response is a gRPC
     * exchange. Set when the initial HttpResponse has content-type starting with
     * "application/grpc". Used to validate grpc-status presence in trailing headers
     * on the H1->H2 translation path. Reset on response completion.
     */
    private boolean translatedStreamIsGrpc;

    /**
     * BUG-14: Set to {@code true} when the backend channel is being closed because
     * the backend sent "Connection: close". In this case, the frontend keep-alive
     * connection must NOT be closed — only the backend channel dies, and the next
     * client request will create a fresh backend connection.
     *
     * <p>Without this flag, the close cascade (channelInactive -> close()) would
     * unconditionally close the inbound (frontend) channel, causing the client to
     * see an unexpected RST on a keep-alive connection.</p>
     *
     * <p>This flag does not need resetting because each {@code DownstreamHandler}
     * instance is bound to exactly one backend channel lifecycle. When the backend
     * closes, this handler is discarded; the next request creates a new handler.</p>
     */
    private boolean backendInitiatedClose;

    DownstreamHandler(HttpConnection httpConnection, Channel inboundChannel, ConnectionPool connectionPool) {
        this.httpConnection = httpConnection;
        isConnectionHttp2 = inboundChannel.pipeline().get(Http2FrameCodec.class) != null;
        this.inboundChannel = inboundChannel;
        this.connectionPool = connectionPool;
        this.maxResponseBodySize = httpConnection.httpConfiguration.maxResponseBodySize();
    }

    /**
     * Detaches this handler from the inbound (client-facing) channel without closing it.
     *
     * <p>This MUST be called before closing an orphaned backend connection whose
     * client-facing channel is still in use (e.g., after a WebSocket upgrade, the
     * original HTTP backend connection is no longer needed but the client channel
     * is now serving WebSocket frames). Without detaching first, the cascading
     * {@link #close()} call from {@code channelInactive} would close the still-active
     * client channel.</p>
     */
    // ME-08: Synchronized to prevent race with close(). Without this, a concurrent close()
    // could capture a non-null inboundChannel reference between detach's intent and execution,
    // then close a channel that should have been preserved (e.g., after WebSocket upgrade).
    synchronized void detachInboundChannel() {
        this.inboundChannel = null;
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) {
        // BUG-ALPN-CTX: Set ctx here in addition to channelActive().
        // When the DownstreamHandler is added to the pipeline by ALPNHandler
        // AFTER TLS+ALPN negotiation completes, the channel is already active.
        // Netty does not fire channelActive() for handlers added after the
        // channel has activated, so this.ctx would remain null, causing NPE
        // when proxyInboundHttp2ToHttp2() accesses ctx.channel().
        this.ctx = ctx;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    /**
     * H2-05 / V-FC-012 / V-FC-021: Backpressure between client and backend.
     *
     * <p>When the backend channel becomes unwritable (its outbound buffer is full),
     * stop reading from the inbound (client) channel to apply TCP-level backpressure.
     * When the backend becomes writable again, resume reading — but only if it is safe
     * to do so.</p>
     *
     * <p><b>H1 frontend (single-stream):</b> The 1:1 toggle is straightforward — when
     * this backend is writable, re-enable frontend autoRead; when unwritable, disable it.</p>
     *
     * <p><b>H2 frontend (multiplexed):</b> A single frontend connection fans out to
     * multiple backend connections. One backend becoming writable must NOT unconditionally
     * re-enable frontend autoRead, because a different backend may still be unwritable.
     * If the frontend resumes reading, DATA frames destined for the unwritable backend
     * pile up in its outbound buffer — exactly the unbounded buffering that V-FC-022
     * prohibits. Therefore, for H2 frontends, autoRead is re-enabled only when ALL
     * active backend channels are writable (V-FC-021).</p>
     *
     * <p>When the backend becomes unwritable, we can immediately disable frontend autoRead
     * without consulting other backends — any one unwritable backend is sufficient to pause.</p>
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        Channel peer = inboundChannel;
        if (peer != null) {
            boolean backendWritable = ctx.channel().isWritable();
            if (isConnectionHttp2 && connectionPool != null) {
                // V-FC-021: For multiplexed H2 frontends, only re-enable autoRead when
                // ALL backend channels are writable. If this backend just became unwritable,
                // immediately pause — no need to check others.
                if (backendWritable) {
                    peer.config().setAutoRead(connectionPool.allBackendsWritable());
                } else {
                    peer.config().setAutoRead(false);
                }
            } else {
                // H1 path: simple 1:1 peer toggle (V-FC-012).
                peer.config().setAutoRead(backendWritable);
            }
        }
        super.channelWritabilityChanged(ctx);
    }

    @Override
    public void channelReadComplete(ChannelHandlerContext ctx) {
        // DEF-DS-01: Flush coalescing for H1 response path. All write() calls in
        // channelRead() are batched and flushed here in a single writev() syscall,
        // reducing per-chunk system call overhead. For H2, the frontend handler's
        // channelReadComplete already handles flushing, so this only flushes the
        // inbound H1 channel.
        final Channel inbound = this.inboundChannel;
        if (inbound != null && !isConnectionHttp2) {
            inbound.flush();
        }
        ctx.fireChannelReadComplete();
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {

        // BUG-07: Capture volatile field into a local variable to eliminate the
        // TOCTOU race between null-check and use. detachInboundChannel() can null
        // the field from another thread between the check and any subsequent read.
        final Channel inbound = this.inboundChannel;

        if (inbound == null) {
            ReferenceCountUtil.release(msg);
        } else if (msg instanceof HttpResponse || msg instanceof HttpContent) {
            // RFC 7230 Section 6.1: Strip hop-by-hop headers from backend responses.
            // Use stripForResponse() to preserve Transfer-Encoding, which Netty's
            // HttpServerCodec needs for correct chunked response encoding.
            if (msg instanceof HttpResponse httpResponse) {
                // H1-07 / GAP-H1-07: Detect "Connection: close" from backend before stripping
                // hop-by-hop headers. RFC 9112 Section 6.1: The Connection header field value
                // is a comma-separated list of tokens (e.g., "keep-alive, close" or just "close").
                // We must parse all tokens to detect "close" rather than doing a simple
                // equalsIgnoreCase check, which would miss "keep-alive, close".
                String connectionHeader = httpResponse.headers().get(HttpHeaderNames.CONNECTION);
                if (connectionHeader != null) {
                    for (String token : connectionHeader.split(",")) {
                        if (token.trim().equalsIgnoreCase("close")) {
                            backendConnectionClose = true;
                            break;
                        }
                    }
                }

                HopByHopHeaders.stripForResponse(httpResponse.headers());
                // RFC 9110 Section 7.6.3: Add Via header to responses
                httpResponse.headers().add(Headers.VIA, "1.1 expressgateway");
            }
            // DEF-H2-02: Enforce configurable response body size limit.
            // Tracks cumulative response body bytes and sends 502 if exceeded.
            if (maxResponseBodySize > 0 && msg instanceof HttpContent httpContent) {
                responseBodyBytes += httpContent.content().readableBytes();
                if (responseBodyBytes > maxResponseBodySize) {
                    logger.warn("DEF-H2-02: Response body size {} exceeds limit {} from backend {}",
                            responseBodyBytes, maxResponseBodySize, httpConnection.node().socketAddress());
                    ReferenceCountUtil.release(msg);
                    // Send 502 Bad Gateway to client and close the backend connection
                    if (isConnectionHttp2) {
                        Http2Headers errorHeaders = new DefaultHttp2Headers();
                        errorHeaders.status("502");
                        // For H2, we'd need stream context — simplified: close backend
                    } else if (!headerRead) {
                        io.netty.handler.codec.http.DefaultFullHttpResponse errorResp =
                                new io.netty.handler.codec.http.DefaultFullHttpResponse(HTTP_1_1,
                                        io.netty.handler.codec.http.HttpResponseStatus.BAD_GATEWAY);
                        errorResp.headers().set(CONTENT_LENGTH, 0);
                        inbound.writeAndFlush(errorResp).addListener(ChannelFutureListener.CLOSE);
                    }
                    ctx.close();
                    return;
                }
            }
            if (isConnectionHttp2) {
                normalizeInboundHttp11ToHttp2(inbound, msg);
            } else {
                // DEF-DS-01: Use write() instead of writeAndFlush() for H1 responses.
                // Flush is deferred to channelReadComplete() for syscall batching —
                // multiple response chunks decoded from the same TCP segment are
                // coalesced into a single writev() call, matching the TCP proxy pattern.
                inbound.write(msg).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);

                // H2-05: If the inbound (client) channel is not writable, stop reading
                // from the backend to propagate backpressure upstream.
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }

                // OBS-01: Record per-backend latency when H1 response completes.
                if (msg instanceof FullHttpResponse || msg instanceof LastHttpContent) {
                    recordBackendLatency();
                }

                // H1-07 + BUG-14: If the backend sent "Connection: close" and the full
                // response has been forwarded (FullHttpResponse or LastHttpContent), close
                // the backend channel. Set backendInitiatedClose BEFORE closing so that
                // the channelInactive -> close() cascade knows to keep the frontend alive.
                if (backendConnectionClose
                        && (msg instanceof FullHttpResponse || msg instanceof LastHttpContent)) {
                    backendConnectionClose = false;
                    backendInitiatedClose = true;
                    ctx.channel().close();
                } else if (!backendConnectionClose
                        && (msg instanceof FullHttpResponse || msg instanceof LastHttpContent)) {
                    // Response complete on a keep-alive H1 backend connection.
                    // Return the connection to the pool for reuse by the next request.
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH1(httpConnection.node(), httpConnection);
                    }
                }
            }
        } else if (msg instanceof Http2HeadersFrame || msg instanceof Http2DataFrame) {
            // RFC 7230 Section 6.1: Strip hop-by-hop headers from backend HTTP/2 responses
            if (msg instanceof Http2HeadersFrame h2HeadersFrame) {
                HopByHopHeaders.strip(h2HeadersFrame.headers());
                // RFC 9110 Section 7.6.3: Add Via header to responses
                h2HeadersFrame.headers().add(Headers.VIA, "2.0 expressgateway");
            }
            if (isConnectionHttp2) {
                proxyInboundHttp2ToHttp2(inbound, (Http2StreamFrame) msg);
            } else {
                normalizeInboundHttp2ToHttp11(inbound, msg);
            }
        } else if (msg instanceof Http2WindowUpdateFrame) {
            // HTTP/2 flow control is per-connection (RFC 9113 Section 6.9).
            // WINDOW_UPDATE frames MUST NOT be forwarded between independent
            // flow-control domains (client<->proxy vs proxy<->backend) as this
            // corrupts window state on both connections.
            ReferenceCountUtil.release(msg);
        } else if (msg instanceof Http2SettingsFrame || msg instanceof Http2PingFrame || msg instanceof Http2SettingsAckFrame) {
            // H2-03: RFC 9113 Section 6.5 — SETTINGS are connection-level and MUST NOT
            // be forwarded between independent connections. Each side negotiates its own
            // settings independently. Release to prevent reference count leaks.
            ReferenceCountUtil.release(msg);
        } else if (msg instanceof Http2GoAwayFrame goAwayFrame) {
            // V-H2-030 / V-H2-031 / CRIT-02: Backend GOAWAY handling.
            //
            // A backend GOAWAY means this backend connection will not accept new streams
            // with IDs above lastStreamId. The proxy MUST:
            //   1. Evict the backend connection from the pool (prevent new stream creation)
            //   2. Prune streams above lastStreamId in the BACKEND stream ID domain
            //   3. Send RST_STREAM(REFUSED_STREAM) to the client for each pruned stream
            //      so clients know to retry those requests
            //   4. Let in-flight streams below lastStreamId complete normally
            //
            // IMPORTANT: Do NOT forward the backend GOAWAY as a client GOAWAY. The client-
            // proxy H2 connection is independent from the proxy-backend connection. A backend
            // going away does not mean the client connection should be terminated -- the client
            // may have streams routed to other backends on the same H2 connection.

            int lastStreamId = goAwayFrame.lastStreamId();
            long errorCode = goAwayFrame.errorCode();

            // CRIT-02 / V-H2-031: Release the original GOAWAY frame immediately.
            // The content ByteBuf is owned by goAwayFrame. Since we are NOT forwarding the
            // GOAWAY to the client (a proxy must not leak backend connection-level events to
            // the independent client connection), there is no need to retain the content.
            // Calling release() on the frame decrements the content ByteBuf's refcount.
            ReferenceCountUtil.release(goAwayFrame);

            // MED-04: Remove streams above lastStreamId using the BACKEND stream ID domain.
            // The lastStreamId from the backend GOAWAY is in the backend's ID space, not the
            // client's. removeStreamsAboveBackendId() correctly iterates the map and compares
            // against each stream's proxyStream().id() (the backend-side stream ID).
            java.util.List<Streams.Stream> pruned =
                    httpConnection.streamPropertyMap().removeStreamsAboveBackendId(lastStreamId);

            // For each pruned stream, send RST_STREAM(REFUSED_STREAM) to the client.
            // RFC 9113 Section 8.1.4: REFUSED_STREAM indicates the stream was not processed
            // and can be safely retried by the client.
            if (isConnectionHttp2 && !pruned.isEmpty()) {
                for (Streams.Stream stream : pruned) {
                    io.netty.handler.codec.http2.DefaultHttp2ResetFrame rstFrame =
                            new io.netty.handler.codec.http2.DefaultHttp2ResetFrame(
                                    io.netty.handler.codec.http2.Http2Error.REFUSED_STREAM);
                    rstFrame.stream(stream.clientStream());
                    httpConnection.decrementActiveStreams();
                    inbound.writeAndFlush(rstFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
            }

            // PB-02: Evict this backend connection from the pool so that
            // ConnectionPool.acquireH2() will not hand it out for new streams.
            // Per RFC 9113 Section 6.8, after GOAWAY the remote peer will not
            // accept new streams with IDs above lastStreamId. Existing in-flight
            // streams below lastStreamId continue to completion on this connection;
            // only new stream creation is prevented.
            if (connectionPool != null) {
                connectionPool.evict(httpConnection);
            }

            logger.debug("V-H2-030: Backend GOAWAY received, lastStreamId={}, errorCode={}, " +
                    "pruned {} streams above lastStreamId, evicted connection from pool",
                    lastStreamId, errorCode, pruned.size());
        } else if (msg instanceof Http2ResetFrame http2ResetFrame) {
            if (isConnectionHttp2) {
                final int streamId = http2ResetFrame.stream().id();
                Streams.Stream stream = httpConnection.streamPropertyMap().removeByBackendId(streamId);
                if (stream == null) {
                    return;
                }
                // DEF-DS-02: Create a fresh RST_STREAM frame instead of mutating the
                // received backend frame. The original frame is consumed by a single reader
                // so mutation is technically safe, but creating a new frame avoids a fragile
                // pattern that could break if frame handling changes (e.g., logging, metrics).
                io.netty.handler.codec.http2.DefaultHttp2ResetFrame clientRst =
                        new io.netty.handler.codec.http2.DefaultHttp2ResetFrame(http2ResetFrame.errorCode());
                clientRst.stream(stream.clientStream());
                // MED-02: Decrement active stream count on backend RST_STREAM
                httpConnection.decrementActiveStreams();
                if (connectionPool != null && httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }

                inbound.writeAndFlush(clientRst).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            }
        } else if (msg instanceof WebSocketFrame) {
            inbound.writeAndFlush(msg).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
        } else {
            ReferenceCountUtil.release(msg);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + msg.getClass().getSimpleName(),
                    HttpResponse.class, HttpContent.class,
                    Http2HeadersFrame.class, Http2DataFrame.class,
                    Http2SettingsFrame.class, Http2PingFrame.class, Http2SettingsAckFrame.class,
                    Http2GoAwayFrame.class,
                    Http2ResetFrame.class,
                    WebSocketFrame.class);
        }
    }

    /**
     * BUG-07: {@code inbound} parameter is the locally-captured snapshot of the
     * volatile {@code inboundChannel} field, eliminating TOCTOU races.
     */
    private void normalizeInboundHttp2ToHttp11(Channel inbound, Object o) throws Http2Exception {
        if (o instanceof Http2HeadersFrame headersFrame) {
            if (headersFrame.isEndStream()) {
                if (headerRead) {
                    HttpHeaders httpHeaders = new DefaultHttpHeaders();
                    HttpConversionUtil.addHttp2ToHttpHeaders(-1, headersFrame.headers(), httpHeaders, HttpVersion.HTTP_1_1, true, false);

                    LastHttpContent lastHttpContent = new DefaultLastHttpContent(Unpooled.EMPTY_BUFFER, httpHeaders);
                    inbound.writeAndFlush(lastHttpContent).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    headerRead = false;
                } else {
                    FullHttpResponse fullHttpResponse = HttpConversionUtil.toFullHttpResponse(-1, headersFrame.headers(),
                            Unpooled.EMPTY_BUFFER, true);
                    inbound.writeAndFlush(fullHttpResponse).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
                // V-FC-012: Backpressure on H2->H1 path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
                // H2->H1 stream completed — decrement active stream count and notify pool.
                recordBackendLatency();
                httpConnection.decrementActiveStreams();
                if (connectionPool != null && httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }
            } else {
                HttpResponse httpResponse = HttpConversionUtil.toHttpResponse(-1, headersFrame.headers(), true);
                inbound.writeAndFlush(httpResponse).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                // V-FC-012: Backpressure on H2->H1 path (non-endStream HEADERS)
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
                headerRead = true;
            }
        } else if (o instanceof Http2DataFrame dataFrame) {
            if (dataFrame.isEndStream()) {
                LastHttpContent lastHttpContent = new DefaultLastHttpContent(dataFrame.content());
                inbound.writeAndFlush(lastHttpContent).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                // V-FC-012: Backpressure on H2->H1 DATA path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
                headerRead = false;
                // H2->H1 stream completed — decrement active stream count and notify pool.
                recordBackendLatency();
                httpConnection.decrementActiveStreams();
                if (connectionPool != null && httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }
            } else {
                HttpContent httpContent = new DefaultHttpContent(dataFrame.content());
                inbound.writeAndFlush(httpContent).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                // V-FC-012: Backpressure on H2->H1 DATA path (non-endStream)
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            }
        } else {
            ReferenceCountedUtil.silentRelease(o);
            throw new UnsupportedMessageTypeException("Unsupported Message: " + o.getClass().getSimpleName(),
                    Http2HeadersFrame.class, Http2DataFrame.class);
        }
    }

    /**
     * BUG-07: {@code inbound} parameter is the locally-captured snapshot of the
     * volatile {@code inboundChannel} field, eliminating TOCTOU races.
     */
    private void proxyInboundHttp2ToHttp2(Channel inbound, Http2StreamFrame streamFrame) {
        final int backendStreamId = streamFrame.stream().id();
        // Use reverse map (backend stream ID -> entry) since frontend and backend
        // stream IDs are decoupled with per-stream connection pooling.
        Streams.Stream stream = httpConnection.streamPropertyMap().getByBackendId(backendStreamId);

        if (stream == null) {
            ReferenceCountUtil.release(streamFrame);
            return;
        }

        streamFrame.stream(stream.clientStream());

        if (streamFrame instanceof Http2HeadersFrame headersFrame) {
            boolean isGrpc = GrpcDetector.isGrpc(headersFrame.headers());

            // Track gRPC streams on the initial response HEADERS (non-endStream)
            if (isGrpc && !headersFrame.isEndStream()) {
                activeGrpcStreamIds.add(backendStreamId);
            }

            // Skip HTTP/2-level compression for gRPC responses; gRPC handles its own encoding.
            if (!isGrpc) {
                applyCompressionOnHttp2(headersFrame.headers(), stream.acceptEncoding());
            }

            if (headersFrame.isEndStream()) {
                // For gRPC streams, ensure grpc-status is present in trailers
                if (activeGrpcStreamIds.remove(backendStreamId)) {
                    // GRPC-TV-01: Per gRPC spec, trailers MUST contain grpc-status.
                    // If the backend sent trailer HEADERS without it, inject
                    // grpc-status: 13 (INTERNAL) — the backend responded but with a
                    // malformed gRPC response. This differs from UNAVAILABLE (14) which
                    // is reserved for cases where the backend is genuinely unreachable.
                    // Preserve grpc-message if the backend already set one.
                    if (!headersFrame.headers().contains(GrpcConstants.GRPC_STATUS)) {
                        headersFrame.headers().set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_INTERNAL);
                        if (!headersFrame.headers().contains(GrpcConstants.GRPC_MESSAGE)) {
                            headersFrame.headers().set(GrpcConstants.GRPC_MESSAGE, "Backend did not send grpc-status");
                        }
                        logger.warn("Synthesized grpc-status=13 for stream {} (missing from backend trailers)", backendStreamId);
                    }
                }
                httpConnection.streamPropertyMap().removeByBackendId(backendStreamId);
                // H2 stream completed — decrement active stream count and notify pool.
                httpConnection.decrementActiveStreams();
                if (connectionPool != null && httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }
            }
        } else if (streamFrame instanceof Http2DataFrame dataFrame) {
            if (dataFrame.isEndStream()) {
                // gRPC DATA endStream means no trailers sent — synthesize grpc-status trailers
                if (activeGrpcStreamIds.remove(backendStreamId)) {
                    Http2FrameStream clientStream = stream.clientStream();
                    Http2DataFrame dataWithoutEnd = new DefaultHttp2DataFrame(dataFrame.content().retain(), false);
                    dataWithoutEnd.stream(clientStream);
                    inbound.write(dataWithoutEnd).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);

                    Http2Headers trailers = new DefaultHttp2Headers();
                    trailers.set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_UNAVAILABLE);
                    trailers.set(GrpcConstants.GRPC_MESSAGE, "Backend closed stream without trailers");
                    inbound.writeAndFlush(new DefaultHttp2HeadersFrame(trailers, true).stream(clientStream));

                    ReferenceCountUtil.release(dataFrame);
                    httpConnection.streamPropertyMap().removeByBackendId(backendStreamId);
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }
                    logger.warn("Synthesized grpc-status=14 trailers for stream {} (backend sent DATA endStream without trailers)", backendStreamId);
                    return;
                }
                httpConnection.streamPropertyMap().removeByBackendId(backendStreamId);
                // H2 stream completed — decrement active stream count and notify pool.
                httpConnection.decrementActiveStreams();
                if (connectionPool != null && httpConnection.node() != null) {
                    connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                }
            }
        }

        inbound.writeAndFlush(streamFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);

        // V-FC-012: Backpressure — if the inbound (client) channel is not writable,
        // stop reading from the backend to propagate backpressure upstream.
        if (!inbound.isWritable()) {
            ctx.channel().config().setAutoRead(false);
        }
    }

    /**
     * BUG-07: {@code inbound} parameter is the locally-captured snapshot of the
     * volatile {@code inboundChannel} field, eliminating TOCTOU races.
     */
    private void normalizeInboundHttp11ToHttp2(Channel inbound, Object o) {
        // B-001: Null-safety guard. lastTranslatedStreamProperty() can return null
        // after clearTranslatedStreamProperty() is called (e.g., race between endStream
        // clearing the field and a late-arriving response chunk). Capture once and check.
        Streams.Stream translatedStream = httpConnection.lastTranslatedStreamProperty();
        if (translatedStream == null) {
            ReferenceCountUtil.release(o);
            return;
        }

        if (o instanceof HttpResponse httpResponse) {
            // GRPC-TV-02: Detect gRPC responses on the H1->H2 translation path by
            // checking content-type. While gRPC natively uses HTTP/2, a backend behind
            // a protocol-translating intermediary could produce H1 with gRPC content-type.
            String ct = httpResponse.headers().get(HttpHeaderNames.CONTENT_TYPE);
            translatedStreamIsGrpc = ct != null
                    && ct.toLowerCase().startsWith(GrpcConstants.CONTENT_TYPE_GRPC_PREFIX);

            Http2Headers http2Headers = toHttp2Headers(httpResponse.headers(), true);
            // XPROTO-04: HttpConversionUtil.toHttp2Headers() only converts regular headers,
            // NOT the :status pseudo-header. We must set it from the H1 response status line.
            // Without this, H2 clients receive PROTOCOL_ERROR ("no status code in response").
            http2Headers.status(httpResponse.status().codeAsText());

            if (httpResponse instanceof FullHttpResponse fullHttpResponse) {
                // F-06: RFC 9113 Section 8.1 — Responses to status codes that MUST NOT
                // contain a message body (1xx, 204, 304 per RFC 9110 Section 6.4.1,
                // Section 15.3.5, Section 15.4.5) should be sent as a single HEADERS
                // frame with END_STREAM set. Sending an empty DATA frame for these
                // statuses violates HTTP/2 semantics and confuses strict clients.
                int statusCode = fullHttpResponse.status().code();
                if (isNoBodyStatus(statusCode)) {
                    // Release the empty content buffer — no DATA frame needed.
                    fullHttpResponse.content().release();

                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    http2HeadersFrame.stream(translatedStream.clientStream());
                    httpConnection.clearTranslatedStreamProperty();
                    // H1->H2 stream completed — decrement active stream count and record latency.
                    recordBackendLatency();
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }

                    inbound.writeAndFlush(http2HeadersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else {
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    http2HeadersFrame.stream(translatedStream.clientStream());

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpResponse.content(), true);
                    dataFrame.stream(translatedStream.clientStream());
                    httpConnection.clearTranslatedStreamProperty();
                    // H1->H2 stream completed — decrement active stream count and record latency.
                    recordBackendLatency();
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }

                    inbound.write(http2HeadersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    inbound.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
                // V-FC-012: Backpressure on H1->H2 FullHttpResponse path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            } else {
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                http2HeadersFrame.stream(translatedStream.clientStream());

                inbound.writeAndFlush(http2HeadersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                // V-FC-012: Backpressure on H1->H2 HttpResponse path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            }
        } else if (o instanceof HttpContent httpContent) {
            if (httpContent instanceof LastHttpContent lastHttpContent) {

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty() && !translatedStreamIsGrpc) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    dataFrame.stream(translatedStream.clientStream());
                    httpConnection.clearTranslatedStreamProperty();
                    // H1->H2 stream completed — decrement active stream count and record latency.
                    recordBackendLatency();
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }

                    inbound.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else if (lastHttpContent.trailingHeaders().isEmpty() && translatedStreamIsGrpc) {
                    // GRPC-TV-03: gRPC stream ended without trailers on H1->H2 path.
                    // Synthesize grpc-status: 13 (INTERNAL) trailers because gRPC clients
                    // require a grpc-status trailer on every response.
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    dataFrame.stream(translatedStream.clientStream());

                    Http2Headers grpcTrailers = new DefaultHttp2Headers();
                    grpcTrailers.set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_INTERNAL);
                    grpcTrailers.set(GrpcConstants.GRPC_MESSAGE, "Backend closed stream without grpc-status trailer");
                    Http2HeadersFrame trailersFrame = new DefaultHttp2HeadersFrame(grpcTrailers, true);
                    trailersFrame.stream(translatedStream.clientStream());

                    httpConnection.clearTranslatedStreamProperty();
                    recordBackendLatency();
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }
                    translatedStreamIsGrpc = false;

                    inbound.write(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    inbound.writeAndFlush(trailersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    logger.warn("GRPC-TV-03: Synthesized grpc-status=13 trailers on H1->H2 path (no trailing headers from backend)");
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    dataFrame.stream(translatedStream.clientStream());

                    Http2Headers http2Headers = toHttp2Headers(lastHttpContent.trailingHeaders(), true);
                    // GRPC-TV-04: On H1->H2 path, validate grpc-status in converted trailers.
                    if (translatedStreamIsGrpc && !http2Headers.contains(GrpcConstants.GRPC_STATUS)) {
                        http2Headers.set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_INTERNAL);
                        if (!http2Headers.contains(GrpcConstants.GRPC_MESSAGE)) {
                            http2Headers.set(GrpcConstants.GRPC_MESSAGE, "Backend did not send grpc-status");
                        }
                        logger.warn("GRPC-TV-04: Synthesized grpc-status=13 in H1->H2 trailing headers");
                    }
                    translatedStreamIsGrpc = false;
                    Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    http2HeadersFrame.stream(translatedStream.clientStream());
                    httpConnection.clearTranslatedStreamProperty();
                    // H1->H2 stream completed — decrement active stream count and record latency.
                    recordBackendLatency();
                    httpConnection.decrementActiveStreams();
                    if (connectionPool != null && httpConnection.node() != null) {
                        connectionPool.releaseH2Stream(httpConnection.node(), httpConnection);
                    }

                    inbound.write(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    inbound.writeAndFlush(http2HeadersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
                // V-FC-012: Backpressure on H1->H2 LastHttpContent path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            } else {
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                dataFrame.stream(translatedStream.clientStream());
                inbound.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                // V-FC-012: Backpressure on H1->H2 HttpContent path
                if (!inbound.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + o.getClass().getSimpleName(),
                    HttpResponse.class, HttpContent.class);
        }
    }

    /**
     * Returns {@code true} if the given HTTP status code MUST NOT include a message body.
     *
     * <p>Per RFC 9110:</p>
     * <ul>
     *   <li>Section 15.2: 1xx (Informational) responses never contain a body.</li>
     *   <li>Section 15.3.5: 204 (No Content) MUST NOT contain a body.</li>
     *   <li>Section 15.4.5: 304 (Not Modified) MUST NOT contain a body.</li>
     * </ul>
     *
     * <p>In HTTP/2, these responses MUST be sent as a single HEADERS frame with
     * END_STREAM set. Sending an additional empty DATA frame is a protocol violation
     * that can confuse strict HTTP/2 clients.</p>
     *
     * @param statusCode the HTTP response status code
     * @return {@code true} if the status forbids a message body
     */
    private static boolean isNoBodyStatus(int statusCode) {
        return (statusCode >= 100 && statusCode < 200) // 1xx Informational
                || statusCode == 204                     // No Content
                || statusCode == 304;                    // Not Modified
    }

    /**
     * OBS-01: Record per-backend latency when a response completes.
     */
    private void recordBackendLatency() {
        long startNanos = httpConnection.requestStartNanos;
        if (startNanos > 0 && httpConnection.node() != null) {
            long latencyMs = (System.nanoTime() - startNanos) / 1_000_000;
            String backend = httpConnection.node().socketAddress().toString();
            StandardEdgeNetworkMetricRecorder.INSTANCE.recordBackendLatency(backend, latencyMs);
            httpConnection.requestStartNanos = 0;
        }
    }

    private void applyCompressionOnHttp2(Http2Headers headers, String acceptEncoding) {
        String targetEncoding = CompressionUtil.checkCompressibleForHttp2(headers, acceptEncoding, httpConnection.httpConfiguration.compressionThreshold());
        if (targetEncoding != null) {
            headers.set(HttpHeaderNames.CONTENT_ENCODING, targetEncoding);
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        // Synthesize grpc-status=14 (UNAVAILABLE) trailers for any active gRPC streams
        // that did not receive proper trailers before the backend disconnected.
        if (isConnectionHttp2 && inboundChannel != null && !activeGrpcStreamIds.isEmpty()) {
            for (int streamId : activeGrpcStreamIds) {
                Streams.Stream stream = httpConnection.streamPropertyMap().removeByBackendId(streamId);
                if (stream != null) {
                    Http2Headers trailers = new DefaultHttp2Headers();
                    trailers.set(GrpcConstants.GRPC_STATUS, GrpcConstants.STATUS_UNAVAILABLE);
                    trailers.set(GrpcConstants.GRPC_MESSAGE, "Backend connection closed");
                    inboundChannel.writeAndFlush(new DefaultHttp2HeadersFrame(trailers, true).stream(stream.clientStream()));
                    logger.warn("Synthesized grpc-status=14 trailers for stream {} (backend disconnected)", streamId);
                }
            }
            activeGrpcStreamIds.clear();
        }
        close();
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        try {
            if (cause instanceof IOException) {
                // H2-09: Log at debug level so operators can correlate connection resets
                // with backend behavior without polluting error logs.
                logger.debug("IOException on connection", cause);
            } else if (cause instanceof ReadTimeoutException) {
                // HIGH-03: Generate a proper 504 Gateway Timeout response instead of RST.
                // ReadTimeoutHandler fires this when the backend fails to respond in time.
                logger.warn("Backend read timeout, sending 504 to client");
                send504GatewayTimeout();
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            close();
        }
    }

    /**
     * HIGH-03: Send a 504 Gateway Timeout response to the client.
     * Handles both H1 and H2 frontend protocols.
     */
    private void send504GatewayTimeout() {
        Channel channel = inboundChannel;
        if (channel == null || !channel.isActive()) {
            return;
        }

        if (isConnectionHttp2) {
            Http2Headers headers = new io.netty.handler.codec.http2.DefaultHttp2Headers();
            headers.status(GATEWAY_TIMEOUT.codeAsText());

            Streams.Stream stream = httpConnection.lastTranslatedStreamProperty();
            if (stream != null) {
                Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(headers, true);
                headersFrame.stream(stream.clientStream());
                channel.writeAndFlush(headersFrame);
            }
        } else {
            io.netty.handler.codec.http.FullHttpResponse response =
                    new io.netty.handler.codec.http.DefaultFullHttpResponse(HTTP_1_1, GATEWAY_TIMEOUT);
            response.headers().set(CONTENT_LENGTH, 0);
            channel.writeAndFlush(response);
        }
    }

    @Override
    public synchronized void close() {
        // Evict this backend connection from the pool — it's dead or closing.
        if (connectionPool != null) {
            connectionPool.evict(httpConnection);
        }

        // BUG-07 / CRIT-01: Capture volatile field into a local inside the synchronized
        // block. Both close() and detachInboundChannel() are synchronized on 'this',
        // preventing a concurrent detach from nulling the field between capture and use.
        final Channel inbound = this.inboundChannel;
        if (inbound != null) {
            if (backendInitiatedClose) {
                // BUG-14: The backend sent "Connection: close" — the backend channel is
                // dying but the frontend keep-alive connection is still valid. Do NOT close
                // the inbound channel. Instead, notify the Http11ServerInboundHandler to
                // release its httpConnection reference so the next request on this
                // keep-alive connection creates a fresh backend connection.
                Http11ServerInboundHandler serverHandler =
                        inbound.pipeline().get(Http11ServerInboundHandler.class);
                if (serverHandler != null) {
                    serverHandler.onBackendConnectionClosed(httpConnection);
                }
            } else {
                // F-10: Guard against double close. channelInactive() and exceptionCaught()
                // both call close(), and Netty's channel.close() on an already-closed channel
                // is a no-op but can trigger spurious log noise or listener re-invocations
                // in edge cases. Check isOpen() to be defensive.
                if (inbound.isOpen()) {
                    inbound.close();
                }
            }
            this.inboundChannel = null;
        }

        // HIGH-04: Capture volatile ctx into local before null-check and use,
        // consistent with the inboundChannel capture pattern above.
        final ChannelHandlerContext localCtx = this.ctx;
        if (localCtx != null) {
            // F-10: Same guard for the backend (outbound) channel.
            if (localCtx.channel().isOpen()) {
                localCtx.channel().close();
            }
            this.ctx = null;
        }
    }
}
