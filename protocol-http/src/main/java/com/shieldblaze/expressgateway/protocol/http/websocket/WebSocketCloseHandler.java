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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.websocketx.CloseWebSocketFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Implements the WebSocket close handshake state machine per RFC 6455 Section 7.
 *
 * <p>This handler intercepts both inbound and outbound {@link CloseWebSocketFrame} messages
 * and enforces the correct close handshake protocol:</p>
 * <ul>
 *   <li><b>Close initiation</b> (outbound): When we send a Close frame, we transition to
 *       {@code CLOSE_SENT} and schedule a timeout. If the peer does not respond with a Close
 *       frame within the timeout, the connection is forcibly closed.</li>
 *   <li><b>Close response</b> (inbound): When we receive a Close frame while in {@code OPEN}
 *       state, we echo a Close frame back (per RFC 6455 Section 5.5.1) and close the connection.
 *       When we receive a Close frame while in {@code CLOSE_SENT}, the handshake is complete
 *       and we close the connection.</li>
 * </ul>
 *
 * <p><b>Status code validation</b> (RFC 6455 Section 7.4): Close frame status codes are validated.
 * Valid codes are 1000-4999, excluding reserved codes 1004, 1005, 1006, and 1015 which MUST NOT
 * appear in a Close frame on the wire.</p>
 *
 * <p>Thread safety: This handler is not sharable. Each channel gets its own instance.
 * All operations occur on the channel's EventLoop thread, so no synchronization is needed.</p>
 *
 * @see <a href="https://datatracker.ietf.org/doc/html/rfc6455#section-7">RFC 6455 Section 7</a>
 */
final class WebSocketCloseHandler extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(WebSocketCloseHandler.class);

    /**
     * Default close handshake timeout in seconds. Per RFC 6455 Section 7.1.1, after sending
     * a Close frame, an endpoint should wait for the peer to send a Close frame in response.
     * If no response arrives within this timeout, the connection is forcibly closed.
     */
    static final long DEFAULT_CLOSE_TIMEOUT_SECONDS = 30;

    /**
     * Close handshake states per RFC 6455 Section 7.1.
     */
    enum CloseState {
        /** Connection is active, no close frames exchanged. */
        OPEN,
        /** We have sent a Close frame, waiting for the peer's Close frame response. */
        CLOSE_SENT,
        /** We have received a Close frame from the peer (and will respond). */
        CLOSE_RECEIVED,
        /** Close handshake is complete; connection is being torn down. */
        CLOSED
    }

    private final long closeTimeoutSeconds;
    private CloseState state = CloseState.OPEN;
    private ScheduledFuture<?> closeTimeoutFuture;

    /**
     * Creates a handler with the default 30-second close handshake timeout.
     */
    WebSocketCloseHandler() {
        this(DEFAULT_CLOSE_TIMEOUT_SECONDS);
    }

    /**
     * Creates a handler with a custom close handshake timeout.
     *
     * @param closeTimeoutSeconds Timeout in seconds to wait for a Close response.
     *                            Must be positive.
     */
    WebSocketCloseHandler(long closeTimeoutSeconds) {
        if (closeTimeoutSeconds <= 0) {
            throw new IllegalArgumentException("closeTimeoutSeconds must be positive: " + closeTimeoutSeconds);
        }
        this.closeTimeoutSeconds = closeTimeoutSeconds;
    }

    /**
     * Returns the current close handshake state. Package-private for testing.
     */
    CloseState state() {
        return state;
    }

    /**
     * Intercepts inbound Close frames from the peer.
     *
     * <p>Per RFC 6455 Section 5.5.1: "If an endpoint receives a Close frame and did not
     * previously send a Close frame, the endpoint MUST send a Close frame in response."</p>
     */
    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof CloseWebSocketFrame closeFrame) {
            handleInboundClose(ctx, closeFrame);
        } else {
            // Non-close frames pass through only if the connection is still open.
            // Once close has been initiated, non-close frames are silently discarded
            // per RFC 6455 Section 7.1.1 (no further data frames after Close).
            if (state == CloseState.OPEN) {
                super.channelRead(ctx, msg);
            } else {
                // After close is initiated, discard non-close frames
                io.netty.util.ReferenceCountUtil.release(msg);
            }
        }
    }

    /**
     * Intercepts outbound Close frames that we are sending to the peer.
     */
    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof CloseWebSocketFrame closeFrame) {
            handleOutboundClose(ctx, closeFrame, promise);
        } else {
            super.write(ctx, msg, promise);
        }
    }

    /**
     * Handles an inbound Close frame received from the peer.
     */
    private void handleInboundClose(ChannelHandlerContext ctx, CloseWebSocketFrame closeFrame) {
        int statusCode = closeFrame.statusCode();
        String reasonText = closeFrame.reasonText();

        // Validate the status code per RFC 6455 Section 7.4
        if (statusCode != -1 && !isValidCloseCode(statusCode)) {
            logger.warn("Received Close frame with invalid status code {}, closing with 1002 (Protocol Error)", statusCode);
            closeFrame.release();
            // RFC 6455 Section 7.4.1: 1002 indicates a protocol error
            sendCloseAndDisconnect(ctx, 1002, "Invalid close code");
            return;
        }

        switch (state) {
            case OPEN -> {
                // Peer initiated the close. We must echo a Close frame back.
                state = CloseState.CLOSE_RECEIVED;
                logger.debug("Received Close frame (code={}, reason='{}'), echoing Close response",
                        statusCode, sanitize(reasonText));

                // HIGH-11: Forward Close frame to the backend before echoing to client.
                // RFC 6455 Section 7.1.1 — the proxy should forward Close to initiate
                // the backend close handshake. ctx.fireChannelRead() sends the frame
                // to WebSocketUpstreamHandler which forwards it to the backend.
                ctx.fireChannelRead(closeFrame.retain());

                // Echo the same status code back. If the frame had no body (statusCode == -1),
                // send an empty Close frame.
                CloseWebSocketFrame response;
                if (statusCode == -1) {
                    response = new CloseWebSocketFrame();
                } else {
                    response = new CloseWebSocketFrame(statusCode, reasonText);
                }
                closeFrame.release();

                // Send the echo response and close the connection after flush.
                ctx.writeAndFlush(response).addListener(ChannelFutureListener.CLOSE);
                state = CloseState.CLOSED;
            }
            case CLOSE_SENT -> {
                // We previously sent a Close and now received the peer's Close response.
                // The close handshake is complete.
                state = CloseState.CLOSED;
                cancelCloseTimeout();
                closeFrame.release();
                logger.debug("Close handshake complete (received peer's Close response, code={})", statusCode);
                ctx.close();
            }
            case CLOSE_RECEIVED, CLOSED -> {
                // Already in close process or closed. Discard duplicate Close frames.
                logger.debug("Discarding duplicate Close frame in state {}", state);
                closeFrame.release();
            }
            default -> {
                logger.warn("Unexpected close state {} when handling inbound Close frame", state);
                closeFrame.release();
            }
        }
    }

    /**
     * Handles an outbound Close frame that we are sending to the peer.
     */
    private void handleOutboundClose(ChannelHandlerContext ctx, CloseWebSocketFrame closeFrame, ChannelPromise promise) throws Exception {
        switch (state) {
            case OPEN -> {
                // We are initiating the close handshake.
                state = CloseState.CLOSE_SENT;
                logger.debug("Sending Close frame (code={}, reason='{}'), starting {}s timeout",
                        closeFrame.statusCode(), closeFrame.reasonText(), closeTimeoutSeconds);

                // Forward the Close frame to the peer
                super.write(ctx, closeFrame, promise);
                ctx.flush();

                // Schedule a timeout: if the peer does not respond with a Close frame
                // within the timeout, forcibly close the connection.
                scheduleCloseTimeout(ctx);
            }
            case CLOSE_RECEIVED -> {
                // This is our echo response to the peer's Close frame. Forward it.
                super.write(ctx, closeFrame, promise);
            }
            case CLOSE_SENT, CLOSED -> {
                // Already sent a Close or already closed. Discard duplicate outbound Close.
                logger.debug("Discarding duplicate outbound Close frame in state {}", state);
                closeFrame.release();
                promise.setSuccess();
            }
            default -> {
                logger.warn("Unexpected close state {} when handling outbound Close frame", state);
                closeFrame.release();
                promise.setSuccess();
            }
        }
    }

    /**
     * Sends a Close frame with the given status code and reason, then closes the connection.
     * Used for error conditions like invalid status codes.
     */
    private void sendCloseAndDisconnect(ChannelHandlerContext ctx, int statusCode, String reason) {
        if (state == CloseState.CLOSED) {
            ctx.close();
            return;
        }
        state = CloseState.CLOSED;
        cancelCloseTimeout();
        CloseWebSocketFrame frame = new CloseWebSocketFrame(statusCode, reason);
        ctx.writeAndFlush(frame).addListener(ChannelFutureListener.CLOSE);
    }

    /**
     * Schedules the close handshake timeout. If the peer does not respond with a Close
     * frame within {@link #closeTimeoutSeconds}, the connection is forcibly closed.
     *
     * <p>Uses the channel's EventLoop for scheduling to avoid thread-safety issues.</p>
     */
    private void scheduleCloseTimeout(ChannelHandlerContext ctx) {
        closeTimeoutFuture = ctx.channel().eventLoop().schedule(() -> {
            if (state == CloseState.CLOSE_SENT) {
                logger.warn("Close handshake timeout after {}s, forcibly closing connection", closeTimeoutSeconds);
                state = CloseState.CLOSED;
                ctx.close();
            }
        }, closeTimeoutSeconds, TimeUnit.SECONDS);
    }

    /**
     * Cancels the close timeout if it is pending.
     */
    private void cancelCloseTimeout() {
        if (closeTimeoutFuture != null) {
            closeTimeoutFuture.cancel(false);
            closeTimeoutFuture = null;
        }
    }

    /**
     * Clean up the timeout on channel inactive to prevent resource leaks.
     */
    @Override
    public void channelInactive(ChannelHandlerContext ctx) throws Exception {
        cancelCloseTimeout();
        state = CloseState.CLOSED;
        super.channelInactive(ctx);
    }

    /**
     * Clean up the timeout when the handler is removed from the pipeline.
     */
    @Override
    public void handlerRemoved(ChannelHandlerContext ctx) throws Exception {
        cancelCloseTimeout();
    }

    /**
     * Validates a WebSocket Close frame status code per RFC 6455 Section 7.4.
     *
     * <p>Valid status codes:</p>
     * <ul>
     *   <li>1000: Normal Closure</li>
     *   <li>1001: Going Away</li>
     *   <li>1002: Protocol Error</li>
     *   <li>1003: Unsupported Data</li>
     *   <li>1007: Invalid frame payload data</li>
     *   <li>1008: Policy Violation</li>
     *   <li>1009: Message Too Big</li>
     *   <li>1010: Mandatory Ext.</li>
     *   <li>1011: Internal Server Error</li>
     *   <li>1012-1014: Reserved for future use (IANA registry, allowed on wire)</li>
     *   <li>3000-4999: Reserved for libraries, frameworks, and applications</li>
     * </ul>
     *
     * <p>Codes that MUST NOT appear in a Close frame on the wire:</p>
     * <ul>
     *   <li>0-999: Not used</li>
     *   <li>1004: Reserved</li>
     *   <li>1005: No Status Rcvd (MUST NOT be set as a status code in a Close frame)</li>
     *   <li>1006: Abnormal Closure (MUST NOT be set as a status code in a Close frame)</li>
     *   <li>1015: TLS handshake failure (MUST NOT be set as a status code in a Close frame)</li>
     *   <li>1016-2999: Reserved for WebSocket extensions, not yet defined</li>
     *   <li>5000+: Undefined</li>
     * </ul>
     *
     * @param code The status code to validate
     * @return {@code true} if the code is valid for use in a Close frame on the wire
     */
    static boolean isValidCloseCode(int code) {
        // RFC 6455 Section 7.4.1: codes 0-999 are not used
        if (code < 1000) {
            return false;
        }

        // Codes 5000+ are undefined
        if (code > 4999) {
            return false;
        }

        // Reserved codes that MUST NOT be sent in a Close frame (Section 7.4.1)
        if (code == 1004 || code == 1005 || code == 1006 || code == 1015) {
            return false;
        }

        // 1016-2999: Reserved for future WebSocket extensions, not yet assigned.
        // Per Section 7.4.2, these require an extension to define them, so they
        // should not appear on the wire unless an extension is negotiated.
        if (code >= 1016 && code <= 2999) {
            return false;
        }

        return true;
    }
}
