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
import com.shieldblaze.expressgateway.protocol.http.ConnectionPool;
import com.shieldblaze.expressgateway.protocol.http.HttpConnection;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http3.DefaultHttp3DataFrame;
import io.netty.handler.codec.http3.DefaultHttp3Headers;
import io.netty.handler.codec.http3.DefaultHttp3HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Map;
import java.util.Set;

/**
 * Handles HTTP/1.1 responses from the backend and translates them to HTTP/3 frames
 * for the frontend QUIC stream channel.
 *
 * <p>This handler is installed in the backend channel's pipeline. It receives decoded
 * HTTP/1.1 messages ({@link HttpResponse}, {@link HttpContent}, {@link LastHttpContent})
 * and converts them to HTTP/3 frames ({@link Http3HeadersFrame}, {@link Http3DataFrame})
 * which are written to the frontend {@link Channel} (a QuicStreamChannel).</p>
 *
 * <p>The translation follows RFC 9110 Section 3.7 (intermediary protocol version
 * translation). Key differences handled:
 * <ul>
 *   <li>HTTP/1.1 status line -> HTTP/3 :status pseudo-header</li>
 *   <li>Hop-by-hop headers (Connection, Keep-Alive, Transfer-Encoding, etc.) are stripped</li>
 *   <li>HTTP/1.1 chunked transfer encoding is consumed by the codec; raw body bytes
 *       are forwarded as HTTP/3 DATA frames</li>
 *   <li>HTTP/1.1 trailing headers -> HTTP/3 trailer HEADERS frame</li>
 * </ul>
 *
 * <p>Connection pool integration: when a response completes normally (LastHttpContent
 * received), the backend {@link HttpConnection} is returned to the {@link ConnectionPool}
 * for reuse by subsequent HTTP/3 streams, unless the response contains
 * {@code Connection: close} or an error occurred. On unexpected backend disconnect,
 * the connection is evicted from the pool.</p>
 *
 * <p>Backpressure: when the frontend QUIC stream becomes unwritable, this handler
 * pauses reads on the backend channel via {@code channelWritabilityChanged} on the
 * inbound (frontend) side. The inverse direction (backend -> frontend writability)
 * is handled in the frontend's {@code Http3ServerHandler.channelWritabilityChanged}.</p>
 */
final class Http3DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(Http3DownstreamHandler.class);

    /**
     * Hop-by-hop header names that MUST be stripped when translating HTTP/1.1
     * responses to HTTP/3 frames. Per RFC 9110 Section 7.6.1 and RFC 9114
     * Section 4.2, HTTP/3 does not use connection-specific header fields.
     */
    private static final Set<String> HOP_BY_HOP_HEADERS = Set.of(
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "trailers",
            "transfer-encoding",
            "upgrade",
            "proxy-connection"
    );

    /**
     * The frontend QUIC stream channel to write HTTP/3 response frames to.
     * This is a {@link io.netty.handler.codec.quic.QuicStreamChannel} representing
     * the single HTTP/3 request/response stream.
     */
    private volatile Channel frontendChannel;

    /**
     * Timestamp (nanoTime) when the request was received. Used to compute
     * end-to-end request latency for observability.
     */
    private final long requestStartNanos;

    /**
     * Set to {@code true} once the response headers have been forwarded.
     * Prevents duplicate HEADERS frames if the backend sends unexpected data.
     */
    private boolean headersForwarded;

    /**
     * The connection pool to return connections to on response completion.
     */
    private final ConnectionPool connectionPool;

    /**
     * The backend Node this connection targets, for pool release/eviction.
     */
    private final Node node;

    /**
     * The backend HttpConnection wrapping the channel, for pool operations.
     */
    private final HttpConnection httpConnection;

    /**
     * Set to {@code true} if the response contains a Connection: close header,
     * indicating the backend wants to close the connection after this response.
     * In this case, we must NOT return the connection to the pool.
     */
    private boolean connectionClose;

    Http3DownstreamHandler(Channel frontendChannel, long requestStartNanos,
                           ConnectionPool connectionPool, Node node,
                           HttpConnection httpConnection) {
        this.frontendChannel = frontendChannel;
        this.requestStartNanos = requestStartNanos;
        this.connectionPool = connectionPool;
        this.node = node;
        this.httpConnection = httpConnection;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        final Channel frontend = this.frontendChannel;

        if (frontend == null || !frontend.isActive()) {
            // Frontend stream was closed (e.g., client cancelled). Release the message
            // to prevent ByteBuf leaks and close the backend connection.
            ReferenceCountUtil.release(msg);
            ctx.close();
            return;
        }

        if (msg instanceof HttpResponse httpResponse) {
            // -- Translate HTTP/1.1 response headers to HTTP/3 HEADERS frame -----------
            //
            // RFC 9114 Section 4.1.2: An HTTP/3 response consists of:
            // 1. A single HEADERS frame containing :status and response headers
            // 2. Zero or more DATA frames containing the response body
            // 3. Optionally, a trailing HEADERS frame
            Http3Headers h3Headers = new DefaultHttp3Headers();
            h3Headers.status(String.valueOf(httpResponse.status().code()));

            HttpHeaders responseHeaders = httpResponse.headers();

            // Check for Connection: close to know whether to pool the connection
            String connHeader = responseHeaders.get(HttpHeaderNames.CONNECTION);
            if ("close".equalsIgnoreCase(connHeader)) {
                connectionClose = true;
            }

            for (Map.Entry<String, String> entry : responseHeaders) {
                String nameLower = entry.getKey().toLowerCase();

                // Strip hop-by-hop headers -- these are HTTP/1.1 connection-level
                // semantics that do not apply to HTTP/3 (RFC 9114 Section 4.2).
                if (HOP_BY_HOP_HEADERS.contains(nameLower)) {
                    continue;
                }

                h3Headers.add(entry.getKey().toLowerCase(), entry.getValue());
            }

            // GAP-H3-06: RFC 9110 Section 7.6.3 — proxy MUST add Via header to responses
            h3Headers.add("via", "1.1 expressgateway");  // 1.1 because backend response is HTTP/1.1

            Http3HeadersFrame h3HeadersFrame = new DefaultHttp3HeadersFrame(h3Headers);
            headersForwarded = true;

            // If this is a full response with no body (e.g., 204, 304, or HEAD response),
            // and the LastHttpContent follows immediately via FullHttpResponse, handle it
            // in the LastHttpContent path below. Don't close the stream prematurely.
            frontend.write(h3HeadersFrame);

            // If this is a FullHttpResponse (both HttpResponse and LastHttpContent),
            // we must handle the body and stream closure here since the else-if
            // LastHttpContent branch below won't execute.
            if (msg instanceof FullHttpResponse fullResponse) {
                if (fullResponse.content().readableBytes() > 0) {
                    forwardBody(frontend, fullResponse);
                }
                // Close the stream — FIN on the QUIC stream
                frontend.writeAndFlush(Unpooled.EMPTY_BUFFER)
                        .addListener(f -> frontend.close());

                // Return connection to pool
                if (connectionClose) {
                    if (connectionPool != null) connectionPool.evict(httpConnection);
                } else {
                    if (connectionPool != null && node != null) connectionPool.releaseH1(node, httpConnection);
                }

                long latencyNanos = System.nanoTime() - requestStartNanos;
                if (logger.isDebugEnabled()) {
                    logger.debug("HTTP/3 request completed (FullHttpResponse) in {}ms", latencyNanos / 1_000_000);
                }
                ReferenceCountUtil.release(msg);
                return;
            } else if (msg instanceof HttpContent httpContent) {
                // HttpResponse with body but not a FullHttpResponse — forward body chunk
                forwardBody(frontend, httpContent);
            }
        } else if (msg instanceof LastHttpContent lastContent) {
            // -- End of HTTP/1.1 response ------------------------------------------------
            //
            // Forward trailing body bytes (if any) as a DATA frame, then forward
            // trailing headers (if any) as a HEADERS frame, then close the stream.
            if (lastContent.content().readableBytes() > 0) {
                Http3DataFrame dataFrame = new DefaultHttp3DataFrame(lastContent.content().retain());
                frontend.write(dataFrame);
            }

            // Forward trailing headers if present
            HttpHeaders trailingHeaders = lastContent.trailingHeaders();
            if (trailingHeaders != null && !trailingHeaders.isEmpty()) {
                Http3Headers h3Trailers = new DefaultHttp3Headers();
                for (Map.Entry<String, String> entry : trailingHeaders) {
                    h3Trailers.add(entry.getKey().toLowerCase(), entry.getValue());
                }
                Http3HeadersFrame trailerFrame = new DefaultHttp3HeadersFrame(h3Trailers);
                frontend.writeAndFlush(trailerFrame)
                        .addListener(ChannelFutureListener.CLOSE);
            } else {
                // No trailers -- flush and close the stream by writing an empty frame
                // and closing. The QuicStreamChannel close sends FIN on the QUIC stream.
                frontend.flush();

                // Close the frontend QUIC stream to signal end-of-response.
                // This sends FIN on the QUIC stream per RFC 9000 Section 3.2.
                frontend.close();
            }

            // Log latency for observability
            long latencyNanos = System.nanoTime() - requestStartNanos;
            if (logger.isDebugEnabled()) {
                logger.debug("HTTP/3 request completed in {}ms", latencyNanos / 1_000_000);
            }

            // -- Return connection to pool or close ------------------------------------
            // If Connection: close was indicated, close the connection.
            // Otherwise, return it to the pool for reuse by future streams.
            if (connectionClose) {
                if (connectionPool != null) {
                    connectionPool.evict(httpConnection);
                }
                // Do NOT close the channel here -- let it drain naturally.
                // The channel will be closed by the idle timeout or ConnectionTimeoutHandler.
            } else {
                if (connectionPool != null && node != null) {
                    connectionPool.releaseH1(node, httpConnection);
                }
            }

            ReferenceCountUtil.release(msg);
        } else if (msg instanceof HttpContent httpContent) {
            // -- Intermediate body chunk -------------------------------------------------
            forwardBody(frontend, httpContent);
            ReferenceCountUtil.release(msg);
        } else {
            // Unexpected message type -- release to prevent leaks
            logger.warn("Unexpected message type from backend: {}", msg.getClass().getName());
            ReferenceCountUtil.release(msg);
        }
    }

    /**
     * Forward an HTTP/1.1 body chunk to the frontend as an HTTP/3 DATA frame.
     *
     * @param frontend the frontend QUIC stream channel
     * @param content  the HTTP content to forward
     */
    private void forwardBody(Channel frontend, HttpContent content) {
        if (content.content().readableBytes() > 0) {
            // DEF-H3DS-01: retain() increments refcount for the Http3DataFrame wrapper.
            // If the write fails (e.g., channel closed), the retained buffer would leak.
            // Add a failure listener that releases the retained content on write error.
            io.netty.buffer.ByteBuf retained = content.content().retain();
            Http3DataFrame dataFrame = new DefaultHttp3DataFrame(retained);
            frontend.write(dataFrame).addListener(future -> {
                if (!future.isSuccess()) {
                    // Release the retained buffer if the write did not succeed —
                    // the pipeline did not consume the frame, so we own the refcount.
                    if (retained.refCnt() > 0) {
                        retained.release();
                    }
                }
            });
        }
    }

    /**
     * Backpressure: when the backend channel becomes unwritable (its outbound
     * buffer to the backend server is full), stop reading from the frontend QUIC
     * stream to prevent request data from accumulating unboundedly.
     * When the backend becomes writable again, resume frontend reads.
     *
     * <p>This is the request-direction backpressure (frontend -> backend).
     * The response-direction backpressure (backend -> frontend) is handled
     * in {@link Http3ServerHandler#channelWritabilityChanged}.</p>
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        Channel frontend = this.frontendChannel;
        if (frontend != null) {
            frontend.config().setAutoRead(ctx.channel().isWritable());
        }
        super.channelWritabilityChanged(ctx);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        // Backend connection closed unexpectedly. Evict from pool instead of returning.
        if (connectionPool != null) {
            connectionPool.evict(httpConnection);
        }

        Channel frontend = this.frontendChannel;
        if (frontend != null && frontend.isActive() && !headersForwarded) {
            // Backend died before sending any response -- send 502 Bad Gateway
            Http3Headers h3Headers = new DefaultHttp3Headers();
            h3Headers.status("502");
            h3Headers.set("content-length", "0");
            Http3HeadersFrame errorFrame = new DefaultHttp3HeadersFrame(h3Headers);
            frontend.writeAndFlush(errorFrame)
                    .addListener(ChannelFutureListener.CLOSE);
        } else if (frontend != null && frontend.isActive()) {
            // Backend closed after partial response -- close the stream.
            // The client will see an incomplete response and may retry.
            frontend.close();
        }

        this.frontendChannel = null;
        super.channelInactive(ctx);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Exception on backend channel (HTTP/3 downstream)", cause);

        // Evict from pool on error -- connection is in unknown state
        if (connectionPool != null) {
            connectionPool.evict(httpConnection);
        }

        Channel frontend = this.frontendChannel;
        if (frontend != null && frontend.isActive() && !headersForwarded) {
            Http3Headers h3Headers = new DefaultHttp3Headers();
            h3Headers.status("502");
            h3Headers.set("content-length", "0");
            Http3HeadersFrame errorFrame = new DefaultHttp3HeadersFrame(h3Headers);
            frontend.writeAndFlush(errorFrame)
                    .addListener(ChannelFutureListener.CLOSE);
        }

        ctx.close();
    }
}
