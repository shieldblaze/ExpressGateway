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
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpgradeProperty;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpstreamHandler;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketPipelineUtils;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpServerExpectContinueHandler;
import io.netty.handler.codec.http.HttpServerKeepAliveHandler;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshaker;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshakerFactory;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;
import static io.netty.handler.codec.http.HttpHeaderNames.CONNECTION;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpHeaderNames.UPGRADE;
import static io.netty.handler.codec.http.HttpResponseStatus.BAD_GATEWAY;
import static io.netty.handler.codec.http.HttpResponseStatus.BAD_REQUEST;
import static io.netty.handler.codec.http.HttpResponseStatus.METHOD_NOT_ALLOWED;
import static io.netty.handler.codec.http.HttpResponseStatus.OK;
import static io.netty.handler.codec.http.HttpResponseStatus.SERVICE_UNAVAILABLE;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;

public class Http11ServerInboundHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(Http11ServerInboundHandler.class);

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final UpstreamRetryHandler retryHandler;
    private final boolean isTLSConnection;
    private final ConnectionPool connectionPool;

    /**
     * The backend connection for the current in-flight request. With per-request
     * load balancing, this is reassigned on every new HttpRequest via
     * {@code connectionPool.acquireH1()} or fresh connection creation.
     */
    private HttpConnection currentRequestConnection;

    /**
     * PIPE-01: HTTP/1.1 pipelining serialization flag.
     *
     * <p>RFC 9112 Section 9.3: "A server MUST send its responses to those
     * [pipelined] requests in the same order that the requests were received."
     * While the proxy already processes requests serially, a fast client can
     * pipeline a second request before the first response is fully written to
     * the client channel. Without serialization, the second request's response
     * headers could interleave with the first response's body chunks, causing
     * protocol corruption.</p>
     *
     * <p>When {@code true}, a request has been forwarded to the backend and we
     * are awaiting the complete response. Any new HttpRequest arriving during
     * this window is held in {@link #pendingPipelinedMsg} rather than processed
     * immediately. When the response completes (detected by the
     * {@link PipelineResponseTracker} seeing a LastHttpContent or FullHttpResponse
     * written to the client), this flag is cleared and the pending request is
     * replayed through {@code channelRead()}.</p>
     */
    private boolean requestInFlight;

    /**
     * PIPE-01: Buffered pipelined request messages awaiting serialization.
     * Stores all messages belonging to the pipelined request (HttpRequest +
     * any HttpContent/LastHttpContent decoded in the same read cycle).
     * At most one request is queued (depth 1). If a second pipelined request
     * arrives while one is already pending, the connection is rejected with 503
     * to bound memory and prevent unbounded queueing from slow clients.
     */
    private List<Object> pendingPipelinedMsgs;

    /**
     * HI-03: Graceful shutdown draining flag. When set to true, this handler will:
     * 1. Reject new requests with 503 Service Unavailable
     * 2. Allow the current in-flight request to complete
     * 3. Close the connection after the response is fully sent
     *
     * <p>Set by {@link #startDraining(ChannelHandlerContext)} during graceful shutdown.</p>
     */
    private volatile boolean draining;

    /**
     * Context reference for graceful shutdown scheduling.
     */
    private ChannelHandlerContext handlerCtx;

    public Http11ServerInboundHandler(HTTPLoadBalancer httpLoadBalancer, boolean isTLSConnection) {
        this.httpLoadBalancer = httpLoadBalancer;
        bootstrapper = new Bootstrapper(httpLoadBalancer);
        retryHandler = new UpstreamRetryHandler(httpLoadBalancer, bootstrapper);
        this.isTLSConnection = isTLSConnection;
        this.connectionPool = new ConnectionPool(httpLoadBalancer.httpConfiguration());
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        this.handlerCtx = ctx;

        // PIPE-01: Install the pipelining response tracker in the pipeline.
        // This handler intercepts outbound writes to detect when a complete response
        // (FullHttpResponse or LastHttpContent) is sent to the client, signaling that
        // the in-flight request is done and a pending pipelined request can proceed.
        //
        // Must be in handlerAdded() (not channelActive()) because in the h2c/cleartext
        // path, H2cHandler adds this handler dynamically to an already-active channel.
        // channelActive() is NOT called for handlers added after the channel is active,
        // but handlerAdded() IS always called.
        ctx.pipeline().addBefore(ctx.name(), "pipelining-tracker", new PipelineResponseTracker(this));

        super.handlerAdded(ctx);
    }

    /**
     * HI-03: Initiate graceful shutdown draining for this HTTP/1.1 connection.
     * New requests on this connection will receive 503 Service Unavailable with
     * Connection: close. The current in-flight request (if any) is allowed to complete.
     *
     * <p>Called from HTTPLoadBalancer.stop() via ConnectionTracker's ChannelGroup iteration.
     * Must be called on the channel's EventLoop thread.</p>
     */
    public void startDraining() {
        draining = true;
        // If there is no in-flight request, close immediately.
        // If there IS an in-flight request, the close will happen after
        // the response completes (Netty's HttpServerKeepAliveHandler or
        // the Connection: close we inject will trigger it).
        if (currentRequestConnection == null && handlerCtx != null && handlerCtx.channel().isActive()) {
            handlerCtx.channel().close();
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        // BUG-011 fix: Check DecoderResult for malformed HTTP/1.1 messages.
        // Netty sets DecoderResult.isFailure() on messages it cannot fully parse
        // (e.g., invalid request line, oversized headers, malformed chunked encoding).
        // Forwarding such messages to the backend would cause undefined behavior.
        if (msg instanceof HttpRequest request && request.decoderResult().isFailure()) {
            Throwable cause = request.decoderResult().cause();
            // RFC 9112: Differentiate error codes based on the specific decoder failure.
            // TooLongHttpLineException → 414 URI Too Long (oversized request line)
            // TooLongHttpHeaderException → 431 Request Header Fields Too Large (oversized headers)
            // Other decoder failures → 400 Bad Request
            HttpResponseStatus status;
            if (cause instanceof io.netty.handler.codec.http.TooLongHttpLineException) {
                status = HttpResponseStatus.REQUEST_URI_TOO_LONG; // 414
                logger.warn("Oversized request line, responding 414: {}", cause.getMessage());
            } else if (cause instanceof io.netty.handler.codec.http.TooLongHttpHeaderException) {
                status = HttpResponseStatus.REQUEST_HEADER_FIELDS_TOO_LARGE; // 431
                logger.warn("Oversized headers, responding 431: {}", cause.getMessage());
            } else {
                status = HttpResponseStatus.BAD_REQUEST; // 400
                logger.warn("Malformed HTTP request, responding 400: {}", cause.getMessage());
            }
            ReferenceCountUtil.release(msg);
            FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, status);
            response.headers().set(CONTENT_LENGTH, 0);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
            response.headers().set(HttpHeaderNames.DATE, httpDate());
            response.headers().add(Headers.VIA, "1.1 expressgateway");
            ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
            return;
        }
        if (msg instanceof HttpContent content && content.decoderResult().isFailure()) {
            logger.warn("Malformed HTTP content, responding 400: {}", content.decoderResult().cause().getMessage());
            ReferenceCountUtil.release(msg);
            FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
            response.headers().set(CONTENT_LENGTH, 0);
            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
            response.headers().set(HttpHeaderNames.DATE, httpDate());
            response.headers().add(Headers.VIA, "1.1 expressgateway");
            ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
            return;
        }

        // PIPE-01: HTTP/1.1 pipelining serialization.
        // RFC 9112 Section 9.3: Responses MUST be sent in the order requests were received.
        // If a request is currently in-flight (awaiting response from backend), buffer the
        // next pipelined request and pause reading from the client. Only one pipelined
        // request is buffered; a second causes a 503 rejection to bound memory.
        if (msg instanceof HttpRequest && requestInFlight) {
            if (pendingPipelinedMsgs != null) {
                // Already have one pending pipelined request -- reject to prevent unbounded queueing.
                logger.warn("PIPE-01: Second pipelined request while one is already pending, responding 503");
                ReferenceCountUtil.release(msg);
                pendingPipelinedMsgs.forEach(ReferenceCountUtil::release);
                pendingPipelinedMsgs = null;
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }
            // Buffer the pipelined request and stop reading from client.
            pendingPipelinedMsgs = new ArrayList<>(2);
            pendingPipelinedMsgs.add(ReferenceCountUtil.retain(msg));
            ctx.channel().config().setAutoRead(false);
            if (logger.isDebugEnabled()) {
                logger.debug("PIPE-01: Buffered pipelined request while previous request is in-flight");
            }
            return;
        }

        // PIPE-01: Buffer HttpContent/LastHttpContent belonging to the pipelined request.
        // When HttpRequest and LastHttpContent are decoded from the same TCP segment,
        // both arrive in the same channelRead batch before autoRead=false takes effect.
        // Without this guard, the LastHttpContent would be written to the current (dying)
        // backend connection, and the replayed request would lack its end-of-message marker.
        if (pendingPipelinedMsgs != null && msg instanceof HttpContent) {
            pendingPipelinedMsgs.add(ReferenceCountUtil.retain(msg));
            return;
        }

        // ME-05: Health check endpoints — respond directly without proxying.
        // /health = liveness: 200 if proxy is running and not draining
        // /ready  = readiness: 200 if proxy has at least one backend node registered
        if (msg instanceof HttpRequest request) {
            String uri = request.uri();
            if ("/health".equals(uri)) {
                ReferenceCountUtil.release(msg);
                HttpResponseStatus status = draining ? SERVICE_UNAVAILABLE : OK;
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, status);
                response.headers().set(CONTENT_LENGTH, 0);
                ctx.writeAndFlush(response);
                return;
            }
            if ("/ready".equals(uri)) {
                ReferenceCountUtil.release(msg);
                boolean ready = !draining && hasHealthyBackend();
                HttpResponseStatus status = ready ? OK : SERVICE_UNAVAILABLE;
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, status);
                response.headers().set(CONTENT_LENGTH, 0);
                ctx.writeAndFlush(response);
                return;
            }
        }

        // HI-03: Reject new requests during graceful shutdown draining.
        // Allow in-flight HttpContent (body chunks) to pass through so the current
        // request can complete, but reject any new HttpRequest.
        if (draining && msg instanceof HttpRequest) {
            ReferenceCountUtil.release(msg);
            FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE);
            response.headers().set(CONTENT_LENGTH, 0);
            response.headers().set(HttpHeaderNames.DATE, httpDate());
            response.headers().set(HttpHeaderNames.CONNECTION, HttpHeaderValues.CLOSE);
            ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
            return;
        }

        // H1-01/H1-02: RFC 9112 Section 3.2 — A server MUST respond with 400 (Bad Request)
        // to any HTTP/1.1 request that lacks a Host header or contains more than one.
        if (msg instanceof HttpRequest request) {
            List<String> hostHeaders = request.headers().getAll(HttpHeaderNames.HOST);
            if (hostHeaders.isEmpty()) {
                logger.warn("Missing Host header, responding 400");
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }
            if (hostHeaders.size() > 1) {
                logger.warn("Multiple Host headers ({}), responding 400", hostHeaders.size());
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // H1-04/SEC-01: RFC 9112 Section 6.1 — Request smuggling defense.
            // A message with both Content-Length and Transfer-Encoding is malformed;
            // reject it to prevent CL/TE desync attacks (cf. HAProxy, Nginx behavior).
            if (request.headers().contains(HttpHeaderNames.CONTENT_LENGTH)
                    && request.headers().contains(HttpHeaderNames.TRANSFER_ENCODING)) {
                logger.warn("Request contains both Content-Length and Transfer-Encoding, responding 400 (smuggling defense)");
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // SEC-03/SEC-04: Validate Content-Length is a strict non-negative integer.
            // RFC 9110 Section 8.6: Content-Length must be a non-negative integer.
            // A negative value or non-numeric string could confuse backend parsers and
            // is a common vector for request smuggling variants.
            //
            // SEC-03 (RS-05): Content-Length with leading/trailing whitespace is suspicious.
            // While Long.parseLong() rejects whitespace with NumberFormatException, Netty's
            // HttpServerCodec may pre-trim the value before it reaches this handler. To
            // provide defense-in-depth, we explicitly require every character to be a digit
            // (0-9). This rejects whitespace, signs, hex digits, and any other non-digit
            // characters that might survive codec processing or be interpreted differently
            // by backend parsers. Per RFC 9110 Section 8.6, the field value is defined as
            // 1*DIGIT — no optional whitespace, no signs.
            String contentLengthStr = request.headers().get(HttpHeaderNames.CONTENT_LENGTH);
            if (contentLengthStr != null) {
                // Strict digit-only check: reject anything that is not purely [0-9]+.
                // This catches whitespace (" 42", "42 "), signs ("-1", "+1"), hex ("0xff"),
                // and any other smuggling-relevant encoding before we even attempt parsing.
                boolean strictlyDigits = !contentLengthStr.isEmpty();
                for (int i = 0, len = contentLengthStr.length(); i < len; i++) {
                    char c = contentLengthStr.charAt(i);
                    if (c < '0' || c > '9') {
                        strictlyDigits = false;
                        break;
                    }
                }
                if (!strictlyDigits) {
                    logger.warn("Content-Length contains non-digit characters: '{}', responding 400", sanitize(contentLengthStr));
                    ReferenceCountUtil.release(msg);
                    FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                    response.headers().set(CONTENT_LENGTH, 0);
                    response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                    response.headers().set(HttpHeaderNames.DATE, httpDate());
                    response.headers().add(Headers.VIA, "1.1 expressgateway");
                    ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                    return;
                }
                // At this point we know the string is all digits. Parse to validate range.
                try {
                    long contentLength = Long.parseLong(contentLengthStr);
                    // This branch is technically unreachable (digits-only string cannot be
                    // negative), but kept as a defensive belt-and-suspenders check.
                    if (contentLength < 0) {
                        logger.warn("Negative Content-Length: {}, responding 400", sanitize(contentLengthStr));
                        ReferenceCountUtil.release(msg);
                        FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                        response.headers().set(CONTENT_LENGTH, 0);
                        response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                        response.headers().set(HttpHeaderNames.DATE, httpDate());
                        response.headers().add(Headers.VIA, "1.1 expressgateway");
                        ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                        return;
                    }
                } catch (NumberFormatException _) {
                    // Can occur for digit-only strings that overflow Long.MAX_VALUE.
                    logger.warn("Content-Length overflows long: {}, responding 400", sanitize(contentLengthStr));
                    ReferenceCountUtil.release(msg);
                    FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                    response.headers().set(CONTENT_LENGTH, 0);
                    response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                    response.headers().set(HttpHeaderNames.DATE, httpDate());
                    response.headers().add(Headers.VIA, "1.1 expressgateway");
                    ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                    return;
                }
            }

            // SEC-04 (RS-06): Validate Transfer-Encoding value is exactly "chunked".
            // RFC 9112 Section 6.1: For HTTP/1.1 requests, the only Transfer-Encoding
            // value a proxy should accept is "chunked". The "identity" coding was
            // deprecated by RFC 7230 and removed entirely in RFC 9112. Compound values
            // like "chunked, identity" or "gzip, chunked" create ambiguity in how
            // different servers interpret body boundaries — a classic smuggling vector.
            //
            // Following HAProxy's strict approach: if Transfer-Encoding is present,
            // the trimmed, lowercased value MUST be exactly "chunked". Anything else
            // (including "identity", "chunked, identity", "gzip", or obfuscated values
            // like "chunKed" with mixed case) is rejected with 400.
            if (request.headers().contains(HttpHeaderNames.TRANSFER_ENCODING)) {
                String teValue = request.headers().get(HttpHeaderNames.TRANSFER_ENCODING);
                if (teValue == null || !teValue.trim().equalsIgnoreCase("chunked")) {
                    logger.warn("Invalid Transfer-Encoding value: '{}', responding 400 (only 'chunked' is accepted)",
                            sanitize(teValue));
                    ReferenceCountUtil.release(msg);
                    FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                    response.headers().set(CONTENT_LENGTH, 0);
                    response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                    response.headers().set(HttpHeaderNames.DATE, httpDate());
                    response.headers().add(Headers.VIA, "1.1 expressgateway");
                    ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                    return;
                }
            }

            // H1-05: Reject CONNECT method — this proxy does not support tunneling.
            // RFC 9110 Section 9.3.6: CONNECT is used for establishing a tunnel, which
            // requires special handling not implemented in this L7 load balancer.
            if (request.method() == HttpMethod.CONNECT) {
                logger.warn("CONNECT method not supported, responding 405");
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, METHOD_NOT_ALLOWED);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.ALLOW, String.join(", ", httpLoadBalancer.httpConfiguration().allowedMethods()));
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // SPEC-2: Block TRACE method — RFC 9110 Section 9.3.8.
            // TRACE echoes the request back to the client which can disclose sensitive
            // hop-by-hop headers (cookies, authorization tokens) that intermediate
            // proxies may have injected. This is a well-known information disclosure
            // vector (cf. Cross-Site Tracing / XST). Both Nginx and HAProxy reject
            // TRACE by default. Respond 405 Method Not Allowed.
            if (request.method() == HttpMethod.TRACE) {
                logger.warn("TRACE method blocked for security, responding 405");
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, METHOD_NOT_ALLOWED);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.ALLOW, String.join(", ", httpLoadBalancer.httpConfiguration().allowedMethods()));
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // SPEC-4: Configurable method restriction per HttpConfiguration.allowedMethods().
            // Only methods in the configured whitelist are forwarded to backends.
            // CONNECT and TRACE are already rejected above; this check covers any
            // additional methods the operator wants to restrict.
            if (!httpLoadBalancer.httpConfiguration().allowedMethods().contains(request.method().name())) {
                logger.warn("Method {} not in allowedMethods, responding 405", request.method());
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, METHOD_NOT_ALLOWED);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.ALLOW, String.join(", ", httpLoadBalancer.httpConfiguration().allowedMethods()));
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // SPEC-3: RFC 9110 Section 7.6.2 — Max-Forwards handling for OPTIONS.
            // A proxy MUST check Max-Forwards before forwarding TRACE or OPTIONS.
            // When Max-Forwards is 0, the proxy MUST NOT forward the request and MUST
            // respond as the final recipient. When Max-Forwards > 0, the proxy MUST
            // decrement it before forwarding. TRACE is already blocked above (SPEC-2),
            // so we only handle OPTIONS here.
            if (request.method() == HttpMethod.OPTIONS) {
                String maxForwardsStr = request.headers().get(HttpHeaderNames.MAX_FORWARDS);
                if (maxForwardsStr != null) {
                    try {
                        int maxForwards = Integer.parseInt(maxForwardsStr.trim());
                        if (maxForwards == 0) {
                            // Max-Forwards reached zero — respond locally as final recipient.
                            logger.debug("OPTIONS with Max-Forwards: 0, responding locally");
                            ReferenceCountUtil.release(msg);
                            FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, OK);
                            response.headers().set(CONTENT_LENGTH, 0);
                            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                            response.headers().set(HttpHeaderNames.ALLOW, String.join(", ", httpLoadBalancer.httpConfiguration().allowedMethods()));
                            response.headers().set(HttpHeaderNames.DATE, httpDate());
                            response.headers().add(Headers.VIA, "1.1 expressgateway");
                            ctx.writeAndFlush(response);
                            return;
                        } else if (maxForwards < 0) {
                            // GAP-H1-05: RFC 9110 Section 7.6.2 — Max-Forwards = 1*DIGIT
                            // (non-negative by grammar). A negative value is malformed.
                            logger.warn("Negative Max-Forwards: {}, responding 400", maxForwards);
                            ReferenceCountUtil.release(msg);
                            FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, BAD_REQUEST);
                            response.headers().set(CONTENT_LENGTH, 0);
                            response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                            response.headers().set(HttpHeaderNames.DATE, httpDate());
                            response.headers().add(Headers.VIA, "1.1 expressgateway");
                            ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                            return;
                        } else {
                            // maxForwards > 0: Decrement Max-Forwards before forwarding to backend.
                            request.headers().setInt(HttpHeaderNames.MAX_FORWARDS, maxForwards - 1);
                        }
                    } catch (NumberFormatException _) {
                        // DEF-H1-01: RFC 9110 Section 7.6.2 — Max-Forwards is 1*DIGIT.
                        // A non-numeric value is malformed; reject with 400.
                        logger.warn("Non-numeric Max-Forwards value: {}, responding 400", sanitize(maxForwardsStr));
                        ReferenceCountUtil.release(msg);
                        FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, BAD_REQUEST);
                        response.headers().set(CONTENT_LENGTH, 0);
                        response.headers().set(HttpHeaderNames.DATE, httpDate());
                        ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                        return;
                    }
                }
            }
        }

        if (msg instanceof HttpRequest request) {
            // BUG-12: RFC 3986 Section 5.2.4 — Normalize the request URI by removing
            // dot segments before forwarding. This provides defense-in-depth against
            // path traversal attacks (e.g., "GET /../../etc/passwd"). A proxy that
            // normalizes ensures backends receive canonical paths regardless of how
            // the client encoded the traversal.
            String normalizedUri = UriNormalizer.normalizeUri(request.uri());
            if (normalizedUri == null) {
                // The URI's ".." segments exceed its path depth, meaning it
                // attempted to traverse above the document root (e.g.,
                // "GET /../../etc/passwd"). This is unambiguously malicious.
                logger.warn("Path traversal detected in URI: {}, responding 400", sanitize(request.uri()));
                ReferenceCountUtil.release(msg);
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.BAD_REQUEST);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }
            request.setUri(normalizedUri);

            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();

            // MED-02: Validate duplicate Content-Length headers with differing values.
            // RFC 9110 Section 8.6: A server MUST reject a request with multiple Content-Length
            // header field values that differ from one another.
            List<String> contentLengths = request.headers().getAll(CONTENT_LENGTH);
            if (contentLengths.size() > 1) {
                String firstValue = contentLengths.getFirst();
                for (int i = 1; i < contentLengths.size(); i++) {
                    if (!firstValue.equals(contentLengths.get(i))) {
                        ReferenceCountUtil.release(msg);
                        FullHttpResponse badRequest = new DefaultFullHttpResponse(HTTP_1_1, BAD_REQUEST);
                        badRequest.headers().set(CONTENT_LENGTH, 0);
                        badRequest.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                        badRequest.headers().set(HttpHeaderNames.DATE, httpDate());
                        badRequest.headers().add(Headers.VIA, "1.1 expressgateway");
                        ctx.writeAndFlush(badRequest).addListener(ChannelFutureListener.CLOSE);
                        return;
                    }
                }
                // Deduplicate: keep only the first value since they all match
                request.headers().set(CONTENT_LENGTH, firstValue);
            }

            Cluster cluster = httpLoadBalancer.cluster(request.headers().getAsString(HttpHeaderNames.HOST));

            // H1-10: If no Cluster was found for that Hostname, respond 503 Service Unavailable.
            // 503 is more appropriate than 502 because the proxy itself is operational but has
            // no backend configured for the requested Host (analogous to Nginx's "no upstream").
            if (cluster == null) {
                ByteBuf responseMessage = ctx.alloc().buffer();
                responseMessage.writeCharSequence(SERVICE_UNAVAILABLE.reasonPhrase(), StandardCharsets.UTF_8);

                FullHttpResponse fullHttpResponse = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE, responseMessage);
                fullHttpResponse.headers().set(CONTENT_LENGTH, responseMessage.readableBytes());
                // BUG-09: Add Content-Type for the text body per RFC 9110 Section 8.3
                fullHttpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                fullHttpResponse.headers().set(HttpHeaderNames.DATE, httpDate());
                // BUG-06: RFC 9110 Section 7.6.3 — proxy-generated errors must include Via
                fullHttpResponse.headers().add(Headers.VIA, "1.1 expressgateway");

                ctx.writeAndFlush(fullHttpResponse).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // Per-request load balancing: select a backend node for EVERY new request.
            // Try to acquire an idle pooled connection first to avoid TCP+TLS overhead.
            {
                Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, request.headers())).node();

                // H1-10/H1-11: 503 Service Unavailable when backend is at capacity.
                if (node.connectionFull()) {
                    FullHttpResponse httpResponse = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE);
                    httpResponse.headers().set(CONTENT_LENGTH, 0);
                    httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                    httpResponse.headers().set(HttpHeaderNames.DATE, httpDate());
                    // BUG-06: RFC 9110 Section 7.6.3 — proxy-generated errors must include Via
                    httpResponse.headers().add(Headers.VIA, "1.1 expressgateway");
                    ctx.writeAndFlush(httpResponse).addListener(ChannelFutureListener.CLOSE);
                    return;
                }

                // Try the pool first — reuse an idle H1 connection to this node.
                HttpConnection pooled = connectionPool.acquireH1(node);
                if (pooled != null) {
                    currentRequestConnection = pooled;
                } else {
                    // No idle connection available — create a new one via retry handler.
                    UpstreamRetryHandler.RetryResult result = retryHandler.attemptWithRetry(
                            cluster, node, ctx.channel(), request, socketAddress, connectionPool);

                    if (result == null) {
                        // H1-11: All connection attempts failed — return 502 Bad Gateway with Content-Length: 0.
                        logger.warn("All backend connection attempts failed for {} {}", request.method(), sanitize(request.uri()));
                        FullHttpResponse httpResponse = new DefaultFullHttpResponse(HTTP_1_1, BAD_GATEWAY);
                        httpResponse.headers().set(CONTENT_LENGTH, 0);
                        httpResponse.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                        httpResponse.headers().set(HttpHeaderNames.DATE, httpDate());
                        // BUG-06: RFC 9110 Section 7.6.3 — proxy-generated errors must include Via
                        httpResponse.headers().add(Headers.VIA, "1.1 expressgateway");
                        ctx.writeAndFlush(httpResponse).addListener(ChannelFutureListener.CLOSE);
                        return;
                    }

                    currentRequestConnection = result.connection();
                    result.node().addConnection(currentRequestConnection);
                }
            }

            // If Upgrade is triggered then don't process this request any further.
            WebSocketUpgradeProperty webSocketProperty = validateWebSocketRequest(ctx, request);
            if (webSocketProperty != null) {
                // Save the node reference before closing the HTTP connection — the
                // WebSocketUpstreamHandler needs it for backend node selection.
                Node wsNode = currentRequestConnection.node();

                // Close the orphaned HTTP backend connection without cascading the close
                // to the client-facing channel. The WebSocket upgrade creates a separate
                // backend connection; the original HTTP connection is no longer needed.
                // Without this, the ReadTimeoutHandler on the orphaned backend channel
                // would fire after the configured timeout, and the DownstreamHandler
                // would cascade-close the still-active client WebSocket channel.
                currentRequestConnection.closeWithoutCascade();
                currentRequestConnection = null;

                // Remove HTTP-specific handlers that are incompatible with WebSocket.
                // handshaker.handshake() may have already removed some of these, so
                // check before removing to avoid NoSuchElementException.
                if (ctx.pipeline().get(HttpContentCompressor.class) != null) {
                    ctx.pipeline().remove(HttpContentCompressor.class);
                }
                if (ctx.pipeline().get(HttpContentDecompressor.class) != null) {
                    ctx.pipeline().remove(HttpContentDecompressor.class);
                }
                if (ctx.pipeline().get(HttpServerKeepAliveHandler.class) != null) {
                    ctx.pipeline().remove(HttpServerKeepAliveHandler.class);
                }
                if (ctx.pipeline().get(HttpServerExpectContinueHandler.class) != null) {
                    ctx.pipeline().remove(HttpServerExpectContinueHandler.class);
                }
                if (ctx.pipeline().get(RequestBodySizeLimitHandler.class) != null) {
                    ctx.pipeline().remove(RequestBodySizeLimitHandler.class);
                }
                if (ctx.pipeline().get(AccessLogHandler.class) != null) {
                    ctx.pipeline().remove(AccessLogHandler.class);
                }
                ctx.pipeline().replace(this, "ws", new WebSocketUpstreamHandler(wsNode, httpLoadBalancer, webSocketProperty));

                // Add WebSocket close handshake (RFC 6455 Section 7) and idle timeout
                // handlers before the upstream handler in the pipeline.
                WebSocketPipelineUtils.addWebSocketHandlers(ctx.pipeline(), "ws");
                return;
            }

            // H1-06: Modify Accept-Encoding, but respect client's explicit identity preference.
            // RFC 9110 Section 12.5.3: If the client sends "Accept-Encoding: identity" or
            // "*;q=0" (rejecting all encodings), do not overwrite — the client explicitly
            // does not want compressed responses.
            String originalAcceptEncoding = request.headers().get(HttpHeaderNames.ACCEPT_ENCODING);
            if (originalAcceptEncoding != null) {
                request.headers().set("X-Original-Accept-Encoding", originalAcceptEncoding);
            }
            if (originalAcceptEncoding == null
                    || (!originalAcceptEncoding.trim().equalsIgnoreCase("identity")
                        && !originalAcceptEncoding.contains("*;q=0"))) {
                // Set supported 'ACCEPT_ENCODING' headers only if client hasn't rejected compression
                request.headers().set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");
            }

            // RFC 7230 Section 6.1: Strip hop-by-hop headers before forwarding to backend
            HopByHopHeaders.strip(request.headers());

            // Use set() instead of add() to prevent client-injected X-Forwarded-* header spoofing.
            // set() replaces any existing values sent by the client.
            request.headers().set(Headers.X_FORWARDED_FOR, socketAddress.getAddress().getHostAddress());
            request.headers().set(Headers.X_FORWARDED_PROTO, isTLSConnection ? "https" : "http");
            request.headers().set(Headers.X_FORWARDED_HOST, request.headers().get(HttpHeaderNames.HOST));
            request.headers().set(Headers.X_FORWARDED_PORT, String.valueOf(((InetSocketAddress) ctx.channel().localAddress()).getPort()));

            // HIGH-02: Set standardized Forwarded header (RFC 7239) using set() to prevent spoofing.
            request.headers().set("forwarded", "for=" + socketAddress.getAddress().getHostAddress()
                    + ";proto=" + (isTLSConnection ? "https" : "http")
                    + ";host=" + request.headers().get(HttpHeaderNames.HOST));

            // GAP-H1-03: RFC 9110 Section 7.6.3 — Via header MUST use the received
            // protocol version, not a hardcoded value. A client sending HTTP/1.0
            // should generate "Via: 1.0 expressgateway", not "Via: 1.1 expressgateway".
            String viaVersion = request.protocolVersion().majorVersion() + "." + request.protocolVersion().minorVersion();
            request.headers().add(Headers.VIA, viaVersion + " expressgateway");

            // SPEC-1: Strip Expect header before forwarding to backend.
            // The HttpServerExpectContinueHandler in the pipeline already sent
            // 100 Continue to the client (or 417 if rejected). If we forward the
            // Expect: 100-continue header to the backend, the backend will also
            // send 100 Continue, resulting in a duplicate interim response reaching
            // the client. RFC 9110 Section 10.1.1: an intermediary that has
            // already satisfied the expectation MUST NOT forward the Expect header.
            //
            // RFC 9110 Section 10.1.1 — 100-Continue Timeout Analysis:
            // A separate 100 Continue timeout for the backend is NOT needed because:
            //   1. The HttpServerExpectContinueHandler (in pipeline before this handler)
            //      already sent "100 Continue" to the client synchronously upon receiving
            //      the Expect header. The client has already started sending its body.
            //   2. The Expect header is stripped below, so the backend never sees it and
            //      never participates in a 100-continue exchange.
            //   3. If the backend is slow to respond after receiving the body, the
            //      ReadTimeoutHandler (configured with backendResponseTimeoutSeconds)
            //      will close the backend connection and trigger a 502/503 to the client.
            //
            // A 100-continue timeout would only be relevant if this proxy forwarded the
            // Expect header to the backend (proxy-to-backend 100-continue). Since we
            // satisfy the expectation locally, there is no inter-hop 100-continue delay.
            request.headers().remove(HttpHeaderNames.EXPECT);

            // REQ-ID: Inject X-Request-ID for end-to-end request correlation.
            // If the client already provided one, preserve it — the client may be
            // chaining through multiple proxies or correlating with its own tracing
            // system. Only generate a new UUID v4 if absent.
            if (!request.headers().contains(Headers.X_REQUEST_ID)) {
                request.headers().set(Headers.X_REQUEST_ID, FastRequestId.generate());
            }

            // PIPE-01: Mark request as in-flight before writing to backend.
            // This ensures any pipelined request arriving on the next read cycle
            // is buffered rather than processed concurrently.
            requestInFlight = true;

            // Write the request to Backend
            try {
                currentRequestConnection.writeAndFlush(request);
            } catch (com.shieldblaze.expressgateway.backend.BacklogOverflowException ex) {
                // BUG-BACKLOG-CRASH: Return 503 instead of crashing the handler.
                logger.warn("Backend backlog overflow, responding 503: {}", ex.getMessage());
                FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE);
                response.headers().set(CONTENT_LENGTH, 0);
                response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                response.headers().set(HttpHeaderNames.DATE, httpDate());
                response.headers().add(Headers.VIA, "1.1 expressgateway");
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                return;
            }
        } else if (msg instanceof HttpContent) {
            if (currentRequestConnection != null) {
                try {
                    currentRequestConnection.writeAndFlush(msg);
                } catch (com.shieldblaze.expressgateway.backend.BacklogOverflowException ex) {
                    logger.warn("Backend backlog overflow on content, responding 503: {}", ex.getMessage());
                    FullHttpResponse response = new DefaultFullHttpResponse(HTTP_1_1, SERVICE_UNAVAILABLE);
                    response.headers().set(CONTENT_LENGTH, 0);
                    response.headers().set(HttpHeaderNames.CONTENT_TYPE, "text/plain; charset=utf-8");
                    response.headers().set(HttpHeaderNames.DATE, httpDate());
                    response.headers().add(Headers.VIA, "1.1 expressgateway");
                    ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                    return;
                }

                // BP-01: Proactive write-side backpressure. After writing body content to
                // the backend, check if its outbound buffer has exceeded the high water mark.
                // If so, pause reading from the client immediately rather than waiting for the
                // async channelWritabilityChanged callback in DownstreamHandler. This limits
                // the amount of data buffered between the check and the event.
                io.netty.channel.Channel backendCh = currentRequestConnection.channel();
                if (backendCh != null && !backendCh.isWritable()) {
                    ctx.channel().config().setAutoRead(false);
                }
            } else {
                // BUG-015: Silently discard HttpContent (including LastHttpContent) when
                // currentRequestConnection is null. This happens when the corresponding
                // HttpRequest was already rejected (400/405/503/etc.) and the response
                // sent with CLOSE listener — or when the backend connection was closed
                // between receiving the request headers and the body content. The HTTP
                // codec continues to emit body chunks (and the trailing LastHttpContent)
                // for the in-progress message even after the handler returned early on
                // the HttpRequest. Throwing UnsupportedMessageTypeException here would
                // kill the connection with an unhandled exception, causing all subsequent
                // pipelined requests to fail and manifesting as cascading timeouts in
                // tests that reuse the connection.
                ReferenceCountUtil.release(msg);
            }
        } else {
            ReferenceCountUtil.release(msg);
            throw new UnsupportedMessageTypeException(msg);
        }
    }

    /**
     * Handles HTTP Protocol Upgrades to WebSocket
     *
     * @param ctx         {@linkplain ChannelHandlerContext} associated with this channel
     * @param httpRequest This {@linkplain HttpRequest}
     * @return Returns {@code true} when an upgrade has happened else {@code false}
     */
    private WebSocketUpgradeProperty validateWebSocketRequest(ChannelHandlerContext ctx, HttpRequest httpRequest) {
        HttpHeaders headers = httpRequest.headers();
        String connection = headers.get(CONNECTION);
        String upgrade = headers.get(UPGRADE);

        if (connection == null || upgrade == null) {
            return null;
        }

        // WS-01: RFC 6455 Section 4.2.1 — Connection header is a token list
        // (e.g., "keep-alive, Upgrade"), so we must check with contains() rather
        // than an exact match. Null safety is guaranteed by the check above.
        if (connection.toLowerCase().contains("upgrade") && "WebSocket".equalsIgnoreCase(upgrade)) {

            // Handshake for WebSocket
            String uri = webSocketURL(httpRequest);
            String subProtocol = httpRequest.headers().get(HttpHeaderNames.SEC_WEBSOCKET_PROTOCOL);
            // WS-03: Disable extension negotiation — backend WebSocket client does not
            // configure extensions, so allowing them on the frontend handshake would
            // create a mismatch where the client expects permessage-deflate but the
            // backend-facing leg does not negotiate it.
            WebSocketServerHandshakerFactory wsFactory = new WebSocketServerHandshakerFactory(uri, subProtocol, false);
            WebSocketServerHandshaker handshaker = wsFactory.newHandshaker(httpRequest);
            if (handshaker == null) {
                WebSocketServerHandshakerFactory.sendUnsupportedVersionResponse(ctx.channel());
            } else {
                handshaker.handshake(ctx.channel(), httpRequest);
            }

            return new WebSocketUpgradeProperty((InetSocketAddress) ctx.channel().remoteAddress(), URI.create(uri), subProtocol, ctx.channel());
        } else {
            return null;
        }
    }

    private String webSocketURL(HttpRequest req) {
        String url = req.headers().get(HttpHeaderNames.HOST) + req.uri();

        // WS-02: The ws/wss scheme depends on whether the CLIENT connected via TLS
        // (i.e., server-side TLS termination), not on whether we originate TLS to
        // the backend (client-side TLS). Use the isTLSConnection flag which is set
        // based on the inbound channel's SSL state.
        if (isTLSConnection) {
            return "wss://" + url;
        }

        return "ws://" + url;
    }

    /**
     * Resume backend reads when the client channel becomes writable again.
     * This completes the backpressure circuit: DownstreamHandler pauses backend
     * reads when the client is unwritable, and this callback resumes them.
     */
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        if (currentRequestConnection != null && ctx.channel().isWritable()) {
            // Re-enable autoRead on the backend channel so responses flow again
            if (currentRequestConnection.channel() != null) {
                currentRequestConnection.channel().config().setAutoRead(true);
            }
        }
        super.channelWritabilityChanged(ctx);
    }

    /**
     * GC-01: Flush coalescing for the HTTP/1.1 frontend-to-backend write path.
     *
     * <p>When Netty delivers a batch of inbound messages (e.g., HttpRequest followed
     * by multiple HttpContent chunks), each {@code channelRead()} call writes to the
     * backend using {@code channel.write()} (no flush). After the entire batch is
     * delivered, Netty calls {@code channelReadComplete()}, at which point we flush
     * all buffered writes to the backend in a single {@code writev()} syscall.</p>
     *
     * <p>This mirrors the HIGH-10 flush coalescing pattern already used in the TCP L4
     * proxy (UpstreamHandler/DownstreamHandler). The benefit: for a chunked POST with
     * N chunks arriving in one read batch, this reduces backend syscalls from N+1
     * (one per writeAndFlush) to 1 (single writev).</p>
     *
     * <p>Safety: If the message is a {@code LastHttpContent} (request complete) or a
     * {@code FullHttpRequest}, the write path in {@link HttpConnection#writeIntoChannel}
     * already uses {@code writeAndFlush()} for that final frame, so the flush here is
     * a harmless no-op (Netty coalesces redundant flushes). For intermediate content
     * chunks that used {@code write()}, this flush ensures they reach the backend
     * promptly even if no more data arrives from the client.</p>
     */
    @Override
    public void channelReadComplete(ChannelHandlerContext ctx) throws Exception {
        if (currentRequestConnection != null) {
            currentRequestConnection.flush();
        }
        super.channelReadComplete(ctx);
    }

    /**
     * BUG-14: Called by {@link DownstreamHandler} when the backend channel closes due to
     * the backend sending {@code Connection: close}. Nulls out the {@code currentRequestConnection}
     * reference so that the next HTTP request on this keep-alive frontend connection will
     * trigger a fresh backend connection via load balancing.
     *
     * <p>This is distinct from {@link #close()} which closes the backend connection.
     * Here, the backend is already closing itself — we just need to forget our reference
     * to it so we do not try to reuse a dead channel.</p>
     *
     * @param closedConn the backend connection that was closed
     */
    void onBackendConnectionClosed(HttpConnection closedConn) {
        connectionPool.evict(closedConn);
        if (currentRequestConnection == closedConn) {
            currentRequestConnection = null;
        }
    }

    /**
     * PIPE-01: Called by {@link PipelineResponseTracker} when a complete HTTP response
     * has been written to the client channel (FullHttpResponse or LastHttpContent).
     * Clears the in-flight flag and replays any buffered pipelined request.
     *
     * <p>This method is always called on the channel's EventLoop thread (from the
     * outbound write path), so no synchronization is needed.</p>
     */
    void onResponseComplete(ChannelHandlerContext ctx) {
        requestInFlight = false;

        if (pendingPipelinedMsgs != null) {
            List<Object> pending = pendingPipelinedMsgs;
            pendingPipelinedMsgs = null;

            if (logger.isDebugEnabled()) {
                logger.debug("PIPE-01: Replaying buffered pipelined request ({} messages)", pending.size());
            }

            // HIGH-01: Guarantee autoRead is re-enabled even if replay throws.
            // Without try-finally, an exception during channelRead() leaves autoRead
            // disabled permanently, stalling the connection.
            int replayed = 0;
            try {
                // Re-enable auto-read before replaying so that body chunks can flow.
                ctx.channel().config().setAutoRead(true);

                for (Object msg : pending) {
                    channelRead(ctx, msg);
                    replayed++;
                }
            } catch (Exception e) {
                logger.error("PIPE-01: Error replaying pipelined request", e);
                // Release only un-consumed messages (those not yet passed to channelRead).
                for (int i = replayed; i < pending.size(); i++) {
                    ReferenceCountUtil.safeRelease(pending.get(i));
                }
                ctx.channel().close();
            } finally {
                // HIGH-01: Belt-and-suspenders — ensure autoRead is always re-enabled.
                // If ctx.channel().close() was called in the catch block, setAutoRead
                // is a no-op on a closed channel, so this is safe.
                if (ctx.channel().isActive()) {
                    ctx.channel().config().setAutoRead(true);
                }
            }
        }
    }

    /**
     * PIPE-01: Outbound handler that intercepts writes to the client channel to detect
     * when an HTTP/1.1 response has been fully sent. When a {@link FullHttpResponse} or
     * {@link io.netty.handler.codec.http.LastHttpContent} is written, it signals the
     * {@link Http11ServerInboundHandler} that the in-flight request is complete.
     *
     * <p>This handler is installed in the pipeline BEFORE the Http11ServerInboundHandler
     * (i.e., closer to the channel/socket) so it sees outbound messages after they have
     * been processed by all upstream handlers.</p>
     */
    private static final class PipelineResponseTracker extends io.netty.channel.ChannelOutboundHandlerAdapter {

        private final Http11ServerInboundHandler serverHandler;

        PipelineResponseTracker(Http11ServerInboundHandler serverHandler) {
            this.serverHandler = serverHandler;
        }

        @Override
        public void write(ChannelHandlerContext ctx, Object msg, io.netty.channel.ChannelPromise promise)
                throws Exception {
            boolean isResponseComplete = msg instanceof FullHttpResponse
                    || msg instanceof io.netty.handler.codec.http.LastHttpContent;

            if (isResponseComplete) {
                // Chain a listener on the promise so we process the pending request
                // AFTER the response bytes are written to the socket, not before.
                // Always call onResponseComplete regardless of success/failure to
                // avoid leaving pendingPipelinedMsgs stuck indefinitely.
                promise.addListener(future -> serverHandler.onResponseComplete(ctx));
            }

            super.write(ctx, msg, promise);
        }
    }

    /**
     * PC-01: Handle TCP half-close (client sent FIN). The client has closed its
     * write side, meaning no more request data will arrive. For HTTP/1.1, this
     * means the request is incomplete — close the connection cleanly rather than
     * hanging indefinitely waiting for more data.
     */
    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        if (evt instanceof io.netty.channel.socket.ChannelInputShutdownEvent) {
            logger.debug("PC-01: Client half-closed connection (FIN received), closing");
            close();
            ctx.channel().close();
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
                // Swallow this harmless IOException
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            // H1-09: Close both the backend connection AND the client channel.
            // Previously only the backend was closed, leaving the client socket
            // lingering in a half-open state.
            close();
            ctx.channel().close();
        }
    }

    /**
     * GAP-H1-06: RFC 9110 Section 6.6.1 — A server MUST send a Date header field
     * in all responses (except 1xx and 5xx when the clock is unreliable). Proxy-generated
     * error responses (400, 405, 413, 414, 431, 502, 503) were missing this header.
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
        } catch (Exception _) {
            return false;
        }
    }

    @Override
    public void close() {
        if (currentRequestConnection != null) {
            currentRequestConnection.close();
            currentRequestConnection = null;
        }
        // PIPE-01: Release any buffered pipelined request messages to prevent ByteBuf leak.
        if (pendingPipelinedMsgs != null) {
            pendingPipelinedMsgs.forEach(ReferenceCountUtil::safeRelease);
            pendingPipelinedMsgs = null;
        }
        requestInFlight = false;
        connectionPool.closeAll();
    }
}
