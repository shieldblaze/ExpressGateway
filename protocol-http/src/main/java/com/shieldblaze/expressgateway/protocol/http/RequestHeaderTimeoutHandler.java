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

import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http.HttpRequest;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * SEC-02: Slowloris defense — enforces a deadline for receiving the first
 * complete set of HTTP request headers on a new connection.
 *
 * <h3>Problem</h3>
 * <p>A Slowloris attacker opens a TCP connection and sends request headers
 * very slowly (e.g., one byte per second). The existing
 * {@link com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler}
 * only fires when a connection is completely idle (zero bytes for N seconds).
 * Because the attacker IS sending data — just slowly — that idle timeout
 * never fires, and the connection is held open indefinitely, exhausting
 * server file descriptors and connection slots.</p>
 *
 * <h3>Solution</h3>
 * <p>This handler starts an absolute-deadline timer when the connection
 * becomes active (or when the handler is added to an active pipeline).
 * If a complete {@link HttpRequest} (which represents the request line +
 * all headers) is not received within the configured timeout, the
 * connection is closed.</p>
 *
 * <p>Once the first complete {@code HttpRequest} arrives, this handler
 * cancels its timer and removes itself from the pipeline — subsequent
 * request pipelining / keep-alive is covered by the existing idle timeout.</p>
 *
 * <h3>HTTP/2 considerations</h3>
 * <p>For HTTP/2 over TLS (h2), the ALPN negotiation path does not install
 * this handler because the H2 frame codec has its own SETTINGS timeout and
 * the TLS handshake itself provides a timeout boundary. For cleartext h2c
 * via prior knowledge, the H2cHandler bypasses the H1 pipeline entirely,
 * so this handler is only active on the H1 code path.</p>
 *
 * <h3>Thread safety</h3>
 * <p>This handler is not {@code @Sharable}. Each channel gets its own
 * instance. The timer callback and all {@code channelRead}/{@code channelActive}
 * calls execute on the same EventLoop thread, so no synchronization is
 * required for the {@code timeoutFuture} field.</p>
 *
 * <p>Analogous to Nginx's {@code client_header_timeout} and HAProxy's
 * {@code timeout http-request}.</p>
 */
public final class RequestHeaderTimeoutHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(RequestHeaderTimeoutHandler.class);

    private final long timeoutSeconds;
    // Not volatile: all access is confined to the channel's EventLoop thread.
    // channelActive, handlerAdded, channelRead, and the scheduled task callback
    // all execute on ctx.executor(), which is the EventLoop.
    private ScheduledFuture<?> timeoutFuture;

    /**
     * Creates a new handler with the specified request header timeout.
     *
     * @param timeoutSeconds Maximum number of seconds to wait for the first
     *                       complete HTTP request headers. Must be positive.
     * @throws IllegalArgumentException if timeoutSeconds is not positive
     */
    public RequestHeaderTimeoutHandler(long timeoutSeconds) {
        if (timeoutSeconds <= 0) {
            throw new IllegalArgumentException("timeoutSeconds must be positive: " + timeoutSeconds);
        }
        this.timeoutSeconds = timeoutSeconds;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        startTimeout(ctx);
        super.channelActive(ctx);
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        // If the handler is added to an already-active channel (e.g., dynamically
        // inserted after H2cHandler falls through to the H1 path), start the timer
        // immediately. If the channel is not yet active, channelActive() will start it.
        if (ctx.channel().isActive()) {
            startTimeout(ctx);
        }
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            // Complete request headers received — Slowloris defeated.
            // Cancel the timer and remove ourselves from the pipeline;
            // the existing ConnectionTimeoutHandler covers idle keep-alive.
            cancelTimeout();

            // Remove ourselves from the pipeline before forwarding the message.
            // This avoids processing subsequent pipelined requests through this handler.
            ctx.pipeline().remove(this);

            // Forward the HttpRequest to the next handler. We use ctx.fireChannelRead()
            // rather than super.channelRead() because we've already removed ourselves
            // from the pipeline — ctx still holds a valid reference to the next handler.
            ctx.fireChannelRead(msg);
            return;
        }

        // Not an HttpRequest (e.g., partial content, ByteBuf before codec decodes).
        // Pass through and keep waiting.
        super.channelRead(ctx, msg);
    }

    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) throws Exception {
        // If the handler is removed externally (e.g., pipeline reconfiguration),
        // ensure we clean up the scheduled timeout to avoid a dangling reference
        // that would fire on a reconfigured pipeline.
        cancelTimeout();
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        // Connection closed before headers arrived — clean up the timer.
        cancelTimeout();
        super.channelInactive(ctx);
    }

    /**
     * Starts the request header deadline timer. Uses {@code ctx.executor().schedule()}
     * which runs the callback on the channel's EventLoop thread, avoiding cross-thread
     * issues. The timer is a one-shot, not periodic — either headers arrive in time
     * or the connection is closed.
     */
    private void startTimeout(ChannelHandlerContext ctx) {
        if (timeoutFuture != null) {
            // Already started (guard against double invocation from handlerAdded + channelActive)
            return;
        }

        timeoutFuture = ctx.executor().schedule(() -> {
            if (ctx.channel().isActive()) {
                logger.warn("SEC-02: Request header timeout ({}s) expired for {} — possible Slowloris attack. Closing connection.",
                        timeoutSeconds, ctx.channel().remoteAddress());
                ctx.close();
            }
        }, timeoutSeconds, TimeUnit.SECONDS);
    }

    /**
     * Cancels the pending timeout if it has not already fired.
     */
    private void cancelTimeout() {
        if (timeoutFuture != null) {
            timeoutFuture.cancel(false);
            timeoutFuture = null;
        }
    }
}
