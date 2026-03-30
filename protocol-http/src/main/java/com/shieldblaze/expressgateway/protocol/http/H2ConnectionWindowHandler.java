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
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2ConnectionHandler;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2LocalFlowController;
import io.netty.handler.codec.http2.Http2Stream;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

/**
 * Adjusts the HTTP/2 connection-level flow control window (stream 0) after the
 * H2 codec has completed its handshake.
 *
 * <p>Per RFC 9113 Section 6.9.2, the default connection-level flow control window
 * is 65535 bytes. Under heavy multiplexing (e.g., 100 concurrent streams), this
 * becomes the bottleneck: the aggregate throughput across all streams is limited to
 * 65535 bytes per RTT. This handler sends a WINDOW_UPDATE on stream 0 to increase
 * the connection window to the configured value.</p>
 *
 * <p>This handler must be placed in the pipeline immediately after the
 * {@link Http2ConnectionHandler} (the H2 codec). It fires once on
 * {@code channelActive} (or immediately in {@code handlerAdded} if the channel
 * is already active) and then removes itself from the pipeline.</p>
 *
 * <h3>RFC 9113 Section 6.9 — WINDOW_UPDATE Validation (handled by Netty)</h3>
 * <p>The following WINDOW_UPDATE validations are enforced by Netty's
 * {@link io.netty.handler.codec.http2.DefaultHttp2ConnectionDecoder} at the codec
 * level, before frames reach any application handler:</p>
 * <ul>
 *   <li><b>Zero increment</b> (Section 6.9): A WINDOW_UPDATE with a flow-control
 *       window increment of 0 MUST be treated as a stream error of type
 *       PROTOCOL_ERROR (for stream-level) or a connection error (for connection-level).
 *       Netty's {@code DefaultHttp2ConnectionDecoder.onWindowUpdateRead()} checks
 *       {@code windowSizeIncrement == 0} and throws
 *       {@code Http2Exception.streamError(PROTOCOL_ERROR)} or
 *       {@code Http2Exception.connectionError(PROTOCOL_ERROR)} accordingly.</li>
 *   <li><b>Window overflow</b> (Section 6.9.1): A change to SETTINGS_INITIAL_WINDOW_SIZE
 *       or a WINDOW_UPDATE that causes the flow-control window to exceed 2^31-1 MUST be
 *       treated as a connection error of type FLOW_CONTROL_ERROR. Netty's
 *       {@link io.netty.handler.codec.http2.DefaultHttp2LocalFlowController} and
 *       {@link io.netty.handler.codec.http2.DefaultHttp2RemoteFlowController} enforce
 *       this limit in their {@code incrementFlowControlWindows()} methods.</li>
 * </ul>
 * <p>No additional application-level WINDOW_UPDATE validation is needed in this gateway.</p>
 *
 * <p>Thread safety: This handler runs on the channel's EventLoop thread.
 * It is not {@code @Sharable} — each connection gets its own instance.</p>
 */
public final class H2ConnectionWindowHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(H2ConnectionWindowHandler.class);

    /**
     * RFC 9113 Section 6.9.2: initial connection flow control window size.
     */
    private static final int DEFAULT_CONNECTION_WINDOW = 65535;

    /**
     * Desired connection-level window size in bytes.
     */
    private final int connectionWindowSize;

    /**
     * @param connectionWindowSize desired connection-level window in bytes.
     *                             Must be >= 65535 (the RFC default).
     */
    public H2ConnectionWindowHandler(int connectionWindowSize) {
        if (connectionWindowSize < DEFAULT_CONNECTION_WINDOW) {
            throw new IllegalArgumentException(
                    "connectionWindowSize (" + connectionWindowSize + ") must be >= " + DEFAULT_CONNECTION_WINDOW);
        }
        this.connectionWindowSize = connectionWindowSize;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        adjustConnectionWindow(ctx);
        super.channelActive(ctx);
    }

    @Override
    public void handlerAdded(ChannelHandlerContext ctx) throws Exception {
        // If the channel is already active (e.g., handler added after connect), adjust now.
        if (ctx.channel().isActive()) {
            adjustConnectionWindow(ctx);
        }
    }

    /**
     * Increases the connection-level flow control window by sending a WINDOW_UPDATE
     * on stream 0. The delta is {@code connectionWindowSize - DEFAULT_CONNECTION_WINDOW}.
     *
     * <p>This works by finding the {@link Http2ConnectionHandler} in the pipeline,
     * accessing its {@link Http2Connection}, and calling
     * {@link Http2LocalFlowController#incrementWindowSize} on the connection stream.
     * Netty's flow controller will automatically send the corresponding WINDOW_UPDATE
     * frame to the peer.</p>
     */
    private void adjustConnectionWindow(ChannelHandlerContext ctx) {
        int delta = connectionWindowSize - DEFAULT_CONNECTION_WINDOW;
        if (delta <= 0) {
            // No adjustment needed — desired size is the RFC default.
            ctx.pipeline().remove(this);
            return;
        }

        // Walk the pipeline to find the H2 connection handler (which owns the Http2Connection).
        Http2ConnectionHandler h2Handler = ctx.pipeline().get(Http2ConnectionHandler.class);
        if (h2Handler == null) {
            logger.warn("H2ConnectionWindowHandler: No Http2ConnectionHandler found in pipeline, " +
                    "cannot adjust connection window. Removing self.");
            ctx.pipeline().remove(this);
            return;
        }

        try {
            Http2Connection connection = h2Handler.connection();
            Http2Stream connectionStream = connection.connectionStream();
            Http2LocalFlowController flowController = connection.local().flowController();

            flowController.incrementWindowSize(connectionStream, delta);

            logger.debug("H2 connection window increased from {} to {} (delta={}) on {}",
                    DEFAULT_CONNECTION_WINDOW, connectionWindowSize, delta, ctx.channel());
        } catch (Http2Exception e) {
            logger.error("Failed to adjust H2 connection-level window size", e);
        }

        // One-shot: remove ourselves from the pipeline.
        ctx.pipeline().remove(this);
    }
}
