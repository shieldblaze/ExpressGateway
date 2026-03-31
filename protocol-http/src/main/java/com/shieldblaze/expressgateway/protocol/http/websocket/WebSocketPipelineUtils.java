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

import io.netty.channel.ChannelPipeline;
import io.netty.handler.timeout.IdleStateHandler;

import java.util.concurrent.TimeUnit;

/**
 * Utility for adding WebSocket close handshake and idle timeout handlers
 * to a Netty {@link ChannelPipeline}.
 *
 * <p>This class centralizes the WebSocket pipeline setup so that both the
 * upstream (client-facing) and downstream (backend-facing) pipelines use
 * the same handler configuration. The handlers are added <b>before</b>
 * the specified target handler so they intercept frames before the
 * upstream/downstream handler processes them.</p>
 *
 * <p>Pipeline order after insertion (reading left-to-right is inbound direction):</p>
 * <pre>
 *   ... -> IdleStateHandler -> WebSocketIdleStateHandler -> WebSocketCloseHandler -> targetHandler -> ...
 * </pre>
 *
 * <p>Thread safety: This class is a stateless utility; all methods are safe to call from any thread.</p>
 */
public final class WebSocketPipelineUtils {

    /** Handler name for the WebSocket idle state detector. */
    static final String WS_IDLE_STATE_HANDLER = "ws-idle-state";

    /** Handler name for the WebSocket idle timeout closer. */
    static final String WS_IDLE_HANDLER = "ws-idle";

    /** Handler name for the WebSocket close handshake state machine. */
    static final String WS_CLOSE_HANDLER = "ws-close";

    private WebSocketPipelineUtils() {
        // Utility class
    }

    /**
     * Adds WebSocket close handshake and idle timeout handlers to the pipeline,
     * inserting them before the specified target handler.
     *
     * <p>Uses default timeouts: 5-minute idle timeout, 30-second close handshake timeout.</p>
     *
     * @param pipeline      The channel pipeline to modify
     * @param beforeHandler The name of the handler before which to insert (typically
     *                      the WebSocket upstream or downstream handler)
     */
    public static void addWebSocketHandlers(ChannelPipeline pipeline, String beforeHandler) {
        addWebSocketHandlers(pipeline, beforeHandler,
                WebSocketIdleStateHandler.DEFAULT_IDLE_TIMEOUT_SECONDS,
                WebSocketCloseHandler.DEFAULT_CLOSE_TIMEOUT_SECONDS);
    }

    /**
     * Adds WebSocket close handshake and idle timeout handlers to the pipeline,
     * inserting them before the specified target handler, with custom timeouts.
     *
     * @param pipeline            The channel pipeline to modify
     * @param beforeHandler       The name of the handler before which to insert
     * @param idleTimeoutSeconds  Idle timeout in seconds (no read or write activity)
     * @param closeTimeoutSeconds Close handshake timeout in seconds
     */
    public static void addWebSocketHandlers(ChannelPipeline pipeline, String beforeHandler,
                                            long idleTimeoutSeconds, long closeTimeoutSeconds) {
        // Netty's IdleStateHandler fires IdleStateEvents when no read/write activity
        // occurs for the configured duration. We use ALL_IDLE (combined read+write)
        // to match the "no frames exchanged" semantic.
        pipeline.addBefore(beforeHandler, WS_IDLE_STATE_HANDLER,
                new IdleStateHandler(0, 0, idleTimeoutSeconds, TimeUnit.SECONDS));

        // Listens for IdleStateEvents and initiates a graceful close with 1001 (Going Away)
        pipeline.addBefore(beforeHandler, WS_IDLE_HANDLER,
                new WebSocketIdleStateHandler());

        // RFC 6455 Section 7 close handshake state machine: validates close codes,
        // echoes close frames, enforces close handshake timeout.
        pipeline.addBefore(beforeHandler, WS_CLOSE_HANDLER,
                new WebSocketCloseHandler(closeTimeoutSeconds));
    }
}
