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

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.configuration.http.HttpConfiguration;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http.websocketx.WebSocketFrame;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2GoAwayFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PingFrame;
import io.netty.handler.codec.http2.Http2ResetFrame;
import io.netty.handler.codec.http2.Http2SettingsAckFrame;
import io.netty.handler.codec.http2.Http2SettingsFrame;
import io.netty.handler.codec.http2.Http2WindowUpdateFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslHandler;
import io.netty.util.ReferenceCountUtil;

import java.util.Deque;
import java.util.concurrent.ConcurrentLinkedDeque;
import java.util.concurrent.atomic.AtomicInteger;

import static io.netty.handler.codec.http.HttpHeaderNames.ACCEPT_ENCODING;

public final class HttpConnection extends Connection {

    private final Streams MAP = new Streams();
    final HttpConfiguration httpConfiguration;

    /**
     * Number of active HTTP/2 streams currently using this backend connection.
     * Used by ConnectionPool to determine if this H2 connection has capacity
     * for additional streams (activeStreams < maxConcurrentStreams).
     *
     * <p>Accessed from: frontend EventLoop (increment on HEADERS routing),
     * backend EventLoop (decrement on response endStream/RST_STREAM),
     * pool scan (read during acquire). AtomicInteger for cross-thread safety.</p>
     */
    private final AtomicInteger activeStreamCount = new AtomicInteger(0);

    /**
     * Tracks whether this H1 connection is currently serving a request.
     * Used by ConnectionPool to distinguish checked-out vs idle H1 connections.
     */
    private volatile boolean inUse;

    /**
     * OBS-01: Timestamp (nanoTime) when the current request was sent to the backend.
     * Set in writeIntoChannel(); read in DownstreamHandler to compute per-backend latency.
     */
    volatile long requestStartNanos;

    /**
     * RES-02: Timestamp (via {@link System#nanoTime()}) when this connection was
     * last returned to the idle pool. Used by the eviction sweep to determine if
     * the connection has exceeded the idle timeout. Set in {@link #markIdle()},
     * cleared to 0 in {@link #markInUse()}. Volatile for cross-thread visibility.
     */
    private volatile long idleSinceNanos;

    /**
     * MEM-01: Timestamp (via {@link System#nanoTime()}) when this connection was created.
     * Used by the eviction sweep to enforce maxConnectionAge — connections older than the
     * configured age are evicted regardless of idle status. This prevents stale TCP connections
     * that may have degraded backend-side state (e.g., server-side connection tracking limits,
     * in-kernel buffer bloat, or DNS changes). Set once during construction.
     */
    private final long createdAtNanos = System.nanoTime();

    /**
     * BUG-008 fix: When proxying H2 frontend to H1 backend, multiple H2 streams
     * may send requests concurrently. Since H1 is serial (one request-response at a time),
     * we queue the stream info and pop when responses arrive. This ensures responses
     * are matched to the correct H2 stream in FIFO order.
     */
    private final Deque<Streams.Stream> translatedStreamQueue = new ConcurrentLinkedDeque<>();

    /**
     * Set to {@code true} if this connection is established on top of HTTP/2 (h2)
     */
    // F-12: Must be volatile — written from CompletableFuture.whenCompleteAsync() callback
    // in processBacklog() and read from writeAndFlush() which may execute on a different
    // EventLoop thread. Without volatile, the reading thread may see a stale 'false' value
    // due to CPU caching / JMM reordering, causing H2 frames to be mis-routed as H1.
    private volatile boolean isConnectionHttp2;

    /**
     * BUG-ALPN-RACE fix: Gate that prevents writeAndFlush() from calling writeIntoChannel()
     * until ALPN negotiation completes. Without this, the frontend thread can see
     * state=CONNECTED_AND_ACTIVE (set by init()) before isConnectionHttp2 is determined,
     * causing writes to take the wrong protocol path (raw H1 to an H2 channel).
     *
     * <p>Must be set BEFORE init() is called (in Bootstrapper) so the happens-before chain
     * guarantees visibility: alpnPending=true HB init() HB state=CONNECTED_AND_ACTIVE.
     * Any thread that reads state=CONNECTED_AND_ACTIVE is guaranteed to see alpnPending=true.</p>
     */
    private volatile boolean alpnPending;

    @NonNull
    public HttpConnection(Node node, HttpConfiguration httpConfiguration) {
        super(node);
        this.httpConfiguration = httpConfiguration;
    }

    /**
     * Mark this connection as waiting for ALPN negotiation. Must be called
     * BEFORE {@link #init(ChannelFuture)} to ensure memory visibility.
     */
    void setAlpnPending(boolean pending) {
        this.alpnPending = pending;
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        ALPNHandler alpnHandler = channelFuture.channel().pipeline().get(ALPNHandler.class);

        if (channelFuture.isSuccess()) {
            if (alpnHandler != null) {
                // BUG-ALPN-RACE fix: alpnPending was set to true BEFORE init() in Bootstrapper.
                // This ensures all writeAndFlush() calls add to the backlog while ALPN is pending,
                // even though state is already CONNECTED_AND_ACTIVE. When ALPN completes, we
                // clear the flag and drain the backlog through the correct protocol path.
                alpnHandler.protocol().whenCompleteAsync((protocol, throwable) -> {
                    if (throwable == null) {
                        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                            isConnectionHttp2 = true;
                        }
                        alpnPending = false;
                        writeBacklog();
                    } else {
                        alpnPending = false;
                        clearBacklog();
                    }
                }, channel.eventLoop());
            } else {
                writeBacklog();
            }
        } else {
            clearBacklog();
        }
    }

    private static final int BACKLOG_DRAIN_BATCH_SIZE = 64;

    @Override
    protected void writeBacklog() {
        // Capture and null the queue atomically to prevent concurrent writes.
        final java.util.concurrent.ConcurrentLinkedQueue<Object> queue = backlogQueue;
        backlogQueue = null;
        backlogSize.set(0);

        if (queue == null || queue.isEmpty()) {
            return;
        }

        drainBatchHttp(queue);
    }

    private void drainBatchHttp(java.util.concurrent.ConcurrentLinkedQueue<Object> queue) {
        int written = 0;
        Object o;
        while ((o = queue.poll()) != null) {
            try {
                writeIntoChannel(o);
            } catch (Exception ex) {
                // Release remaining backlog messages on failure to avoid leaks.
                queue.forEach(ReferenceCountUtil::release);
                queue.clear();
                throw new IllegalStateException(ex);
            }
            written++;

            if (written >= BACKLOG_DRAIN_BATCH_SIZE) {
                if (channel != null) {
                    channel.flush();
                }

                if (channel != null && !channel.isWritable() && !queue.isEmpty()) {
                    // Outbound buffer is full — install a one-shot handler that
                    // resumes draining when channelWritabilityChanged fires.
                    channel.pipeline().addLast(new HttpBacklogDrainHandler(queue));
                    return;
                }
                written = 0;
            }
        }

        // Final flush for the trailing partial batch.
        if (written > 0 && channel != null) {
            channel.flush();
        }
    }

    private final class HttpBacklogDrainHandler extends io.netty.channel.ChannelInboundHandlerAdapter {
        private final java.util.concurrent.ConcurrentLinkedQueue<Object> queue;

        HttpBacklogDrainHandler(java.util.concurrent.ConcurrentLinkedQueue<Object> queue) {
            this.queue = queue;
        }

        @Override
        public void channelWritabilityChanged(io.netty.channel.ChannelHandlerContext ctx) throws Exception {
            if (ctx.channel().isWritable()) {
                ctx.pipeline().remove(this);
                drainBatchHttp(queue);
            }
            ctx.fireChannelWritabilityChanged();
        }

        @Override
        public void channelInactive(io.netty.channel.ChannelHandlerContext ctx) throws Exception {
            Object msg;
            while ((msg = queue.poll()) != null) {
                ReferenceCountUtil.release(msg);
            }
            ctx.pipeline().remove(this);
            ctx.fireChannelInactive();
        }
    }

    @NonNull
    @Override
    public void writeAndFlush(Object o) {
        if (state == State.INITIALIZED || alpnPending) {
            // Either not connected yet, or connected but ALPN still pending.
            // Add to backlog; it will be drained when the connection is fully ready.
            java.util.concurrent.ConcurrentLinkedQueue<Object> queue = backlogQueue;
            if (queue != null) {
                int currentSize = backlogSize.incrementAndGet();
                if (currentSize > maxBacklogSize) {
                    backlogSize.decrementAndGet();
                    ReferenceCountUtil.release(o);
                    throw new com.shieldblaze.expressgateway.backend.BacklogOverflowException(
                            "Backlog queue capacity exceeded: " + maxBacklogSize);
                }
                queue.add(o);
            } else {
                // BUG-BACKLOG-RACE: The backlog queue has been nulled by writeBacklog()
                // but state has not yet transitioned to CONNECTED_AND_ACTIVE (both happen
                // on the backend EventLoop; this thread sees state=INITIALIZED because the
                // volatile write of CONNECTED_AND_ACTIVE hasn't been published yet, and
                // backlogQueue is not volatile so we may see null before the state flip).
                //
                // At this point the connection IS active (writeBacklog() already drained
                // the queue and wrote to the channel). Write directly instead of dropping
                // the message, which would cause the POST body to be silently lost —
                // the H1 backend waits forever for Content-Length bytes that never arrive,
                // and the client times out.
                if (channel != null && channel.isActive()) {
                    try {
                        writeIntoChannel(o);
                    } catch (Exception ex) {
                        ReferenceCountUtil.release(o);
                    }
                } else {
                    ReferenceCountUtil.release(o);
                }
            }
        } else if (state == State.CONNECTED_AND_ACTIVE && channel != null) {
            try {
                writeIntoChannel(o);
            } catch (Exception ex) {
                throw new IllegalStateException(ex);
            }
        } else {
            ReferenceCountUtil.release(o);
        }
    }

    private void writeIntoChannel(Object o) throws Http2Exception {
        // OBS-01: Record request start time for per-backend latency tracking.
        if (o instanceof HttpRequest || o instanceof Http2HeadersFrame) {
            requestStartNanos = System.nanoTime();
        }
        // If connection protocol is HTTP/2 and request is HTTP/1.1 then convert the request to HTTP/2.
        //
        // If connection protocol is HTTP/2 and request is HTTP/2 then proxy it.
        if (o instanceof HttpRequest || o instanceof HttpContent) {
            if (isConnectionHttp2) {
                proxyOutboundHttp11ToHttp2(o);
            } else {
                // BUG-01 fix: Do NOT strip hop-by-hop headers or apply compression here
                // for the H1->H1 path. Http11ServerInboundHandler already performs both
                // operations before calling writeAndFlush(). Doing it again would:
                //   1. Waste CPU on a no-op scan of already-stripped headers.
                //   2. Risk removing headers that were legitimately added after the first
                //      strip (e.g., Via, X-Forwarded-*).

                // GC-01: Flush coalescing for H1->H1 proxy path.
                // Use write() for intermediate body chunks and writeAndFlush() only
                // for the final frame (LastHttpContent). The frontend handler's
                // channelReadComplete() calls flush() to ensure any buffered writes
                // are flushed at the end of the read batch. This reduces writev()
                // syscalls from one-per-message to one-per-batch.
                //
                // HttpRequest and non-last HttpContent use write(); LastHttpContent
                // (which may also be a FullHttpRequest implementing LastHttpContent)
                // uses writeAndFlush() to guarantee prompt delivery of the complete
                // request to the backend.
                if (o instanceof LastHttpContent) {
                    channel.writeAndFlush(o).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else {
                    channel.write(o).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
            }
        } else if (o instanceof Http2HeadersFrame || o instanceof Http2DataFrame) {
            if (isConnectionHttp2) {
                proxyOutboundHttp2ToHttp2(o);
            } else {
                proxyOutboundHttp2ToHttp11(o);
            }
        } else if (o instanceof Http2SettingsFrame || o instanceof Http2PingFrame || o instanceof Http2SettingsAckFrame) {
            // H2-03: RFC 9113 Section 6.5 — SETTINGS are connection-level and MUST NOT
            // be forwarded between independent connections. PING and SETTINGS_ACK are also
            // handled locally by each endpoint's Http2FrameCodec. Release to prevent leaks.
            ReferenceCountUtil.release(o);
        } else if (o instanceof Http2GoAwayFrame goAwayFrame) {
            // Client GOAWAY -> forward to backend.
            // If Connection is HTTP/2 then send GOAWAY to the backend server.
            // Else close the HTTP/1.1 connection.
            if (isConnectionHttp2) {
                // CRIT-02 / V-H2-031: The incoming goAwayFrame was created by
                // Http2ServerInboundHandler using retainedDuplicate() on the original
                // client GOAWAY's debug data. We forward it directly to the backend
                // channel -- Netty's Http2FrameCodec will encode and release it.
                //
                // We do NOT re-retain/re-wrap in a new frame because:
                //   1. The caller already created a properly owned frame for us
                //   2. setExtraStreamIds() is relative to the connection's last-known
                //      stream ID, not an absolute value -- the client's lastStreamId is
                //      meaningless in the backend's stream ID space
                //
                // Use extraStreamIds(0) so the backend sees a GOAWAY with lastStreamId
                // equal to the highest stream we've opened on this backend connection.
                goAwayFrame.setExtraStreamIds(0);

                channel.writeAndFlush(goAwayFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            } else {
                ReferenceCountUtil.release(goAwayFrame);
                close();
            }
        } else if (o instanceof Http2WindowUpdateFrame) {
            // HTTP/2 flow control is per-connection (RFC 9113 Section 6.9).
            // WINDOW_UPDATE frames MUST NOT be forwarded between independent
            // flow-control domains (client<->proxy vs proxy<->backend) as this
            // corrupts window state on both connections.
            ReferenceCountUtil.release(o);
        } else if (o instanceof Http2ResetFrame http2ResetFrame) {
            if (isConnectionHttp2) {
                final int streamId = http2ResetFrame.stream().id();
                Streams.Stream stream = streamPropertyMap().remove(streamId);
                if (stream == null) {
                    return;
                }
                http2ResetFrame.stream(stream.proxyStream());

                channel.writeAndFlush(http2ResetFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            }
        } else if (o instanceof WebSocketFrame) {
            channel.writeAndFlush(o).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + o.getClass().getSimpleName(),
                    HttpRequest.class, HttpContent.class,
                    Http2HeadersFrame.class, Http2DataFrame.class,
                    Http2SettingsFrame.class, Http2PingFrame.class, Http2SettingsAckFrame.class,
                    Http2GoAwayFrame.class,
                    Http2ResetFrame.class,
                    WebSocketFrame.class
            );
        }
    }

    private void proxyOutboundHttp2ToHttp11(Object o) throws Http2Exception {
        if (o instanceof Http2HeadersFrame headersFrame) {
            // Apply compression
            applySupportedCompressionHeaders(headersFrame.headers());
            // RFC 7230 Section 6.1: Remove hop-by-hop headers before forwarding
            HopByHopHeaders.strip(headersFrame.headers());

            if (headersFrame.isEndStream()) {
                FullHttpRequest fullHttpRequest = HttpConversionUtil.toFullHttpRequest(-1, headersFrame.headers(), Unpooled.EMPTY_BUFFER, true);
                // XPROTO: Remove x-http2-stream-id injected by HttpConversionUtil (stream -1 is meaningless
                // to an H1 backend and can confuse test backends that branch on this header).
                fullHttpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());

                // BUG-008 fix: Enqueue translatedStreamQueue in the writeAndFlush listener, NOT
                // before the write. When proxyOutboundHttp2ToHttp11() is called from the frontend
                // EventLoop (BUG-BACKLOG-RACE path), addLast() on the frontend thread races with
                // addLast() on the backend EventLoop (backlog drain). The queue order diverges
                // from the wire order, causing H1 responses to map to the wrong H2 streams.
                //
                // The write listener always fires on the backend channel's EventLoop. Since
                // Netty EventLoop is single-threaded and writes on the same channel complete in
                // FIFO order, addLast() in the listener preserves wire order exactly. The listener
                // fires before the backend response arrives (network RTT), so the entry is always
                // present when DownstreamHandler reads it via peekFirst().
                final Streams.Stream streamEntry = new Streams.Stream(null,
                        headersFrame.stream(), headersFrame.stream());
                channel.writeAndFlush(fullHttpRequest).addListener(f -> {
                    if (f.isSuccess()) {
                        translatedStreamQueue.addLast(streamEntry);
                    } else {
                        channel.close();
                    }
                });
            } else {
                HttpRequest httpRequest = HttpConversionUtil.toHttpRequest(-1, headersFrame.headers(), true);
                // XPROTO: Remove synthetic stream ID header (see endStream path above)
                httpRequest.headers().remove(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());

                // BUG-008 fix: Same write-ordering fix as the endStream path above.
                final Streams.Stream streamEntry = new Streams.Stream(httpRequest.headers().get(ACCEPT_ENCODING),
                        headersFrame.stream(), headersFrame.stream());
                channel.writeAndFlush(httpRequest).addListener(f -> {
                    if (f.isSuccess()) {
                        translatedStreamQueue.addLast(streamEntry);
                    } else {
                        channel.close();
                    }
                });
            }
        } else if (o instanceof Http2DataFrame dataFrame) {

            HttpContent httpContent;
            if (dataFrame.isEndStream()) {
                httpContent = new DefaultLastHttpContent(dataFrame.content());
            } else {
                httpContent = new DefaultHttpContent(dataFrame.content());
            }

            // GC-01: Flush coalescing for H2->H1 DATA frames.
            // Final body (endStream) uses writeAndFlush for prompt delivery.
            // Intermediate chunks use write(); the frontend's channelReadComplete()
            // flushes at batch end. writeBacklog() also flushes after draining.
            if (dataFrame.isEndStream()) {
                channel.writeAndFlush(httpContent).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            } else {
                channel.write(httpContent).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + o.getClass().getSimpleName(),
                    Http2HeadersFrame.class, Http2DataFrame.class);
        }
    }

    /**
     * Proxy {@link Http2HeadersFrame} and {@link Http2DataFrame}
     */
    private void proxyOutboundHttp2ToHttp2(Object o) {
        if (o instanceof Http2HeadersFrame headersFrame) {
            // Apply compression
            CharSequence clientAcceptEncoding = headersFrame.headers().get(ACCEPT_ENCODING);
            applySupportedCompressionHeaders(headersFrame.headers());
            // RFC 7230 Section 6.1: Remove hop-by-hop headers before forwarding
            HopByHopHeaders.strip(headersFrame.headers());

            // F-08: RFC 9113 Section 8.3.1 — The :scheme pseudo-header must reflect the
            // scheme of the backend connection, not the original client connection.
            Http2Headers http2Headers = headersFrame.headers();
            boolean isTLSConnection = channel.pipeline().get(SslHandler.class) != null;
            http2Headers.scheme(isTLSConnection ? "https" : "http");

            final int frontendStreamId = headersFrame.stream().id();
            // Save the frontend stream reference BEFORE replacing it on the frame.
            Http2FrameStream clientStream = headersFrame.stream();
            // Auto-increment backend stream ID. With per-stream connection pooling,
            // frontend and backend stream IDs are decoupled.
            Http2FrameStream proxyStream = newFrameStream();
            headersFrame.stream(proxyStream);

            Streams.Stream entry = new Streams.Stream(String.valueOf(clientAcceptEncoding), clientStream, proxyStream);
            // Store by frontend stream ID for outbound DATA frame remapping.
            streamPropertyMap().put(frontendStreamId, entry);

            // BUG-PB01: Register the reverse mapping (backend stream ID -> entry) in the
            // writeAndFlush listener, NOT before the write. In Netty 4.2.x, newStream()
            // returns an Http2FrameStream with id=-1 (unassigned). The actual stream ID
            // is allocated by Http2FrameCodec when the HEADERS frame is written to the
            // wire. Registering before the write stored -1 as the key, causing all
            // backend responses (which arrive on the real stream ID, e.g. 3) to miss
            // the mapping and be silently dropped — the root cause of all 18 TLS test
            // timeout failures.
            //
            // Race safety: the write listener fires on the backend channel's EventLoop
            // thread. The backend's response HEADERS also arrive on that same EventLoop
            // thread. Since Netty EventLoop is single-threaded, the write listener is
            // guaranteed to execute before the next channelRead (the response). There is
            // no race between putByBackendId and getByBackendId.
            channel.writeAndFlush(headersFrame).addListener(f -> {
                if (f.isSuccess()) {
                    streamPropertyMap().putByBackendId(proxyStream.id(), entry);
                } else {
                    // ML-06: Write failed — clean up the forward map entry.
                    streamPropertyMap().remove(frontendStreamId);
                }
            });
        } else if (o instanceof Http2DataFrame dataFrame) {
            final int streamId = dataFrame.stream().id();
            Streams.Stream stream = streamPropertyMap().get(streamId);
            if (stream == null) {
                ReferenceCountUtil.release(dataFrame);
                return;
            }
            dataFrame.stream(stream.proxyStream());

            // GC-01: Flush coalescing for H2->H2 DATA frames.
            // Final frame (endStream) uses writeAndFlush; intermediate uses write().
            // Frontend channelReadComplete() and writeBacklog() both flush buffered writes.
            if (dataFrame.isEndStream()) {
                channel.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            } else {
                channel.write(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + o.getClass().getSimpleName(),
                    Http2HeadersFrame.class, Http2DataFrame.class);
        }
    }

    /**
     * Proxy {@link HttpRequest} and {@link HttpContent}
     */
    private void proxyOutboundHttp11ToHttp2(Object o) {
        if (o instanceof HttpRequest httpRequest) {
            String clientAcceptEncoding = httpRequest.headers().get(ACCEPT_ENCODING);
            // RFC 7230 Section 6.1: Remove hop-by-hop headers before conversion to HTTP/2
            HopByHopHeaders.strip(httpRequest.headers());
            boolean isTLSConnection = channel.pipeline().get(SslHandler.class) != null;

            // XPROTO: Set the scheme extension header BEFORE conversion so that
            // HttpConversionUtil.toHttp2Headers() can find it. Without this, the
            // converter throws ":scheme must be specified" because hop-by-hop stripping
            // in Http11ServerInboundHandler already removed the original scheme header.
            httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(),
                    isTLSConnection ? "https" : "http");

            Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(httpRequest, true);
            http2Headers.scheme(isTLSConnection ? "https" : "http");

            // HIGH-01: RFC 9113 Section 8.3.1 — :authority MUST be present for http/https.
            if (http2Headers.authority() == null) {
                String host = httpRequest.headers().get(io.netty.handler.codec.http.HttpHeaderNames.HOST);
                if (host != null) {
                    http2Headers.authority(host);
                }
            }

            Http2FrameStream frameStream = newFrameStream();

            // Apply compression
            applySupportedCompressionHeaders(http2Headers);

            if (httpRequest instanceof FullHttpRequest fullHttpRequest) {
                // HIGH-06: If body is empty (typical for GET/HEAD/DELETE), send a single
                // HEADERS frame with endStream=true instead of HEADERS + empty DATA.
                if (!fullHttpRequest.content().isReadable()) {
                    // MED-14: Add stream entry to translatedStreamQueue so that
                    // DownstreamHandler can map the backend response back to the
                    // correct frontend stream. Without this, the response is silently
                    // dropped because lastTranslatedStreamProperty() returns null.
                    translatedStreamQueue.addLast(new Streams.Stream(clientAcceptEncoding, frameStream, frameStream));
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    headersFrame.stream(frameStream);
                    fullHttpRequest.content().release();
                    channel.writeAndFlush(headersFrame);
                } else {
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                    headersFrame.stream(frameStream);

                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(fullHttpRequest.content(), true);
                    dataFrame.stream(frameStream);

                // BUG-10 fix: Push stream entry so DownstreamHandler can map the backend
                // response back to the correct H2 frame stream. Without this,
                // lastTranslatedStreamProperty() returns null and the response is silently dropped.
                translatedStreamQueue.addLast(new Streams.Stream(clientAcceptEncoding, frameStream, frameStream));

                channel.write(headersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                channel.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }
            } else {
                Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, false);
                http2HeadersFrame.stream(frameStream);
                channel.writeAndFlush(http2HeadersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);

                // There are HttpContent in queue to process, so we will store this FrameStream for further use.
                translatedStreamQueue.addLast(new Streams.Stream(clientAcceptEncoding, frameStream, frameStream));
            }
        } else if (o instanceof HttpContent httpContent) {
            // B-001: Null-safety guard. The queue can be empty if the HEADERS
            // frame was an endStream (no body expected) or if it was already cleared.
            Streams.Stream currentStream = translatedStreamQueue.peekLast();
            if (currentStream == null) {
                ReferenceCountUtil.release(httpContent);
                return;
            }

            if (httpContent instanceof LastHttpContent lastHttpContent) {

                // > If Trailing Headers are empty then we'll write HTTP/2 Data Frame with 'endOfStream' set to 'true.
                // > If Trailing Headers are present then we'll write HTTP/2 Data Frame followed by HTTP/2 Header Frame which will have 'endOfStream' set to 'true.
                if (lastHttpContent.trailingHeaders().isEmpty()) {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), true);
                    dataFrame.stream(currentStream.proxyStream());

                    channel.writeAndFlush(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                } else {
                    Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                    dataFrame.stream(currentStream.proxyStream());

                    Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(lastHttpContent.trailingHeaders(), true);
                    Http2HeadersFrame headersFrame = new DefaultHttp2HeadersFrame(http2Headers, true);
                    headersFrame.stream(currentStream.proxyStream());

                    channel.write(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                    channel.writeAndFlush(headersFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
                }

                // In H1->H2 direction, we pop the last entry since it's the current in-flight.
                // (H1 frontend is serial, so only one request at a time.)
                translatedStreamQueue.pollLast();
            } else {
                // GC-01: Intermediate body chunk — use write() for flush coalescing.
                Http2DataFrame dataFrame = new DefaultHttp2DataFrame(httpContent.content(), false);
                dataFrame.stream(currentStream.proxyStream());
                channel.write(dataFrame).addListener(ChannelFutureListener.CLOSE_ON_FAILURE);
            }
        } else {
            ReferenceCountUtil.release(o);
            throw new UnsupportedMessageTypeException("Unsupported Object: " + o.getClass().getSimpleName(),
                    HttpRequest.class, HttpContent.class);
        }
    }

    private static void applySupportedCompressionHeaders(Object o) {
        // BUG-02 fix: Respect client's explicit "identity" or "*;q=0" Accept-Encoding.
        // RFC 9110 Section 12.5.3: "identity" means the client does not want any
        // content-coding applied. "*;q=0" rejects all encodings. In both cases we
        // MUST NOT overwrite the header — doing so would cause the backend to compress
        // a response the client cannot (or chose not to) decompress.
        if (o instanceof HttpHeaders headers) {
            String original = headers.get(ACCEPT_ENCODING);
            if (original != null) {
                headers.set("X-Original-Accept-Encoding", original);
            }
            if (!clientRejectsCompression(original)) {
                headers.set(ACCEPT_ENCODING, "br, gzip, deflate");
            }
        } else if (o instanceof Http2Headers headers) {
            CharSequence original = headers.get(ACCEPT_ENCODING);
            if (original != null) {
                headers.set("x-original-accept-encoding", original);
            }
            if (!clientRejectsCompression(original != null ? original.toString() : null)) {
                headers.set(ACCEPT_ENCODING, "br, gzip, deflate");
            }
        }
    }

    /**
     * Returns {@code true} if the client's Accept-Encoding value indicates that
     * compression should not be applied.
     *
     * <p>Per RFC 9110 Section 12.5.3:</p>
     * <ul>
     *   <li>{@code "identity"} — the client explicitly requests no content-coding.</li>
     *   <li>{@code "*;q=0"} — the client rejects all encodings not explicitly listed
     *       (and none are listed, so nothing is acceptable except identity).</li>
     *   <li>{@code null} — no header present; the proxy may advertise its supported
     *       encodings to the backend, so compression is acceptable.</li>
     * </ul>
     *
     * @param acceptEncoding the original Accept-Encoding header value from the client
     * @return {@code true} if the client rejects compression
     */
    private static boolean clientRejectsCompression(String acceptEncoding) {
        if (acceptEncoding == null) {
            return false;
        }
        String trimmed = acceptEncoding.trim();
        if (trimmed.equalsIgnoreCase("identity")) {
            return true;
        }
        // Check for "*;q=0" as a standalone directive (the wildcard with quality 0
        // rejects all encodings). We must avoid false positives on values like
        // "gzip;q=0.5, *;q=0.1" where "*;q=0" is a substring but the actual
        // quality is non-zero. Split on comma, trim each directive, and check
        // if any directive matches "*;q=0" exactly (ignoring whitespace around ";").
        for (String directive : trimmed.split(",")) {
            String d = directive.trim();
            // Normalize whitespace around the semicolon: "* ; q=0" -> "*;q=0"
            String normalized = d.replaceAll("\\s*;\\s*", ";");
            if (normalized.equalsIgnoreCase("*;q=0")) {
                return true;
            }
        }
        return false;
    }

    private Http2FrameStream newFrameStream() {
        return channel.pipeline().get(Http2ChannelDuplexHandler.class).newStream();
    }

    /**
     * Peek at the head of the translated stream queue without removing it.
     * Used by DownstreamHandler to determine which H2 stream to route the response to.
     */
    public Streams.Stream lastTranslatedStreamProperty() {
        return translatedStreamQueue.peekFirst();
    }

    /**
     * Remove the head of the translated stream queue, indicating the current
     * H1 response has been fully forwarded back to the H2 client stream.
     */
    public void clearTranslatedStreamProperty() {
        translatedStreamQueue.pollFirst();
    }

    public Streams streamPropertyMap() {
        return MAP;
    }

    /**
     * Returns {@code true} if this connection uses the HTTP/2 protocol.
     * <p>
     * This is determined during ALPN negotiation when the backend connection
     * is established. The value is only reliable after the connection reaches
     * {@link State#CONNECTED_AND_ACTIVE}.
     */
    @Override
    public boolean isHttp2() {
        return isConnectionHttp2;
    }

    /**
     * Send a GOAWAY frame with NO_ERROR to the remote peer, signaling graceful shutdown
     * per RFC 9113 Section 6.8. The peer MUST NOT open new streams after receiving this
     * frame, but in-flight streams may complete.
     *
     * <p>This method is a no-op if the connection is not HTTP/2 or the channel is not active.</p>
     *
     * @return {@link ChannelFuture} for the write, or {@code null} if no GOAWAY was sent
     *         (non-HTTP/2 connection, null channel, or inactive channel).
     */
    @Override
    public ChannelFuture sendGoaway() {
        if (!isConnectionHttp2 || channel == null || !channel.isActive()) {
            return null;
        }
        // RFC 9113 Section 6.8: A server initiates graceful shutdown by sending
        // GOAWAY with the last stream ID it is willing to accept. NO_ERROR (0x0)
        // indicates this is an intentional, clean shutdown — not an error condition.
        return channel.writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.NO_ERROR));
    }

    /**
     * Closes this backend connection without cascading the close to the client-facing
     * (inbound) channel.
     *
     * <p>This is required during WebSocket upgrades: the proxy establishes an HTTP
     * backend connection to perform cluster/node selection, but the WebSocket upgrade
     * then creates a separate WebSocket backend connection. The original HTTP backend
     * connection becomes orphaned. If closed normally, the {@link DownstreamHandler}
     * on its pipeline would cascade-close the still-active client channel (which is
     * now serving WebSocket frames).</p>
     *
     * <p>This method detaches the {@link DownstreamHandler} from the inbound channel
     * first, then closes the backend channel, preventing the cascade.</p>
     */
    void closeWithoutCascade() {
        // Use channelFuture.channel() instead of the `channel` field because the
        // `channel` field is set asynchronously in a listener and may not be populated
        // yet when this method is called from the frontend EventLoop thread. The
        // channelFuture's channel reference is available immediately after bootstrap.connect().
        Channel ch = channelFuture != null ? channelFuture.channel() : channel;
        if (ch != null) {
            DownstreamHandler downstream = ch.pipeline().get(DownstreamHandler.class);
            if (downstream != null) {
                downstream.detachInboundChannel();
            }
            // Close the backend channel directly via the always-available ChannelFuture
            // reference, since Connection.close() uses the `channel` field which may
            // not be set yet.
            ch.close();
        }
        // Also remove from node and clear backlog
        node().removeConnection(this);
        if (backlogQueue != null && !backlogQueue.isEmpty()) {
            clearBacklog();
        }
    }

    int incrementActiveStreams() {
        int newCount = activeStreamCount.incrementAndGet();
        // OBS-03: Record active H2 stream gauge for this backend node.
        recordActiveStreamMetric(newCount);
        return newCount;
    }

    int decrementActiveStreams() {
        // Guard against underflow: if a gRPC deadline fires and the response also
        // arrives, both paths call decrementActiveStreams(). The CAS loop ensures
        // the count never goes below zero, preventing misleading negative metrics
        // and incorrect pool capacity decisions.
        int current;
        int newCount;
        do {
            current = activeStreamCount.get();
            if (current <= 0) {
                return 0; // Already at zero — avoid underflow
            }
            newCount = current - 1;
        } while (!activeStreamCount.compareAndSet(current, newCount));
        // OBS-03: Record active H2 stream gauge for this backend node.
        recordActiveStreamMetric(newCount);
        return newCount;
    }

    /**
     * OBS-03: Update the per-backend active H2 streams gauge in the global metric recorder.
     * Skips recording if the node is null (e.g., during connection setup before node assignment).
     */
    private void recordActiveStreamMetric(int count) {
        Node n = node();
        if (n != null) {
            StandardEdgeNetworkMetricRecorder.INSTANCE.recordActiveH2Streams(
                    n.socketAddress().toString(), count);
        }
    }

    int activeStreams() {
        return activeStreamCount.get();
    }

    boolean hasStreamCapacity(int maxConcurrentStreams) {
        return activeStreamCount.get() < maxConcurrentStreams;
    }

    /**
     * CM-D1 FIX: Atomically claim a stream slot if capacity is available.
     * Uses CAS to prevent the TOCTOU race where hasStreamCapacity() returns true
     * but another thread increments the count before this thread does, exceeding
     * maxConcurrentStreams.
     *
     * @param maxConcurrentStreams the maximum allowed concurrent streams
     * @return {@code true} if a slot was successfully claimed, {@code false} if at capacity
     */
    boolean tryIncrementActiveStreams(int maxConcurrentStreams) {
        int current;
        do {
            current = activeStreamCount.get();
            if (current >= maxConcurrentStreams) {
                return false;
            }
        } while (!activeStreamCount.compareAndSet(current, current + 1));
        recordActiveStreamMetric(current + 1);
        return true;
    }

    void markInUse() {
        idleSinceNanos = 0;
        inUse = true;
    }

    void markIdle() {
        inUse = false;
        idleSinceNanos = System.nanoTime();
    }

    boolean isInUse() {
        return inUse;
    }

    long idleSinceNanos() {
        return idleSinceNanos;
    }

    /**
     * MEM-01: Returns the nanoTime at which this connection was created.
     * Used by ConnectionPool to enforce maxConnectionAge eviction.
     */
    long createdAtNanos() {
        return createdAtNanos;
    }

    @Override
    public String toString() {
        return "HTTPConnection{" + "isConnectionHttp2=" + isConnectionHttp2
                + ", activeStreams=" + activeStreamCount.get()
                + ", Connection=" + super.toString() + '}';
    }
}
