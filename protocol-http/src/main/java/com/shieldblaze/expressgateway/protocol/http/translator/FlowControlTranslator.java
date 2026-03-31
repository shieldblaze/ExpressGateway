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
package com.shieldblaze.expressgateway.protocol.http.translator;

import io.netty.channel.Channel;
import io.netty.channel.ChannelConfig;

/**
 * Translates flow control signals across HTTP/1.1 (TCP), HTTP/2, and HTTP/3 (QUIC).
 *
 * <h3>Flow control semantics per protocol</h3>
 *
 * <h4>HTTP/1.1 (TCP backpressure)</h4>
 * Flow control is implicit: TCP window size governs how much data the sender can
 * transmit before receiving ACKs. When the Netty channel becomes unwritable (outbound
 * buffer exceeds high water mark), the proxy stops reading from the source channel
 * via {@code channel.config().setAutoRead(false)}. When writable again, autoRead is
 * re-enabled. This is pure TCP backpressure -- no application-level flow control.
 *
 * <h4>HTTP/2 (connection + stream flow control, RFC 9113 Section 6.9)</h4>
 * Two levels of flow control windows:
 * <ul>
 *   <li><b>Connection level:</b> Shared across all streams on the connection. Controlled
 *       by WINDOW_UPDATE on stream 0. Default initial window: 65,535 bytes.</li>
 *   <li><b>Stream level:</b> Per-stream window. Controlled by WINDOW_UPDATE on the
 *       stream's ID. Default initial window: 65,535 bytes (from SETTINGS).</li>
 * </ul>
 * DATA frames decrement both windows. WINDOW_UPDATE increments them. When a window
 * reaches zero, the sender MUST NOT send DATA frames on that stream (or any stream
 * if connection window is zero). Netty's Http2FlowController handles this internally.
 *
 * <h4>HTTP/3 (QUIC flow control, RFC 9000 Section 4)</h4>
 * Two levels of flow control (similar to HTTP/2 but at the QUIC transport layer):
 * <ul>
 *   <li><b>Connection level:</b> MAX_DATA frame controls total data across all streams.</li>
 *   <li><b>Stream level:</b> MAX_STREAM_DATA frame controls data on individual streams.</li>
 * </ul>
 * QUIC flow control is transparent to the HTTP/3 layer -- the QUIC stack handles
 * window management. When a QUIC stream's send buffer is full, the channel becomes
 * unwritable (same Netty abstraction as TCP).
 *
 * <h3>Translation strategy</h3>
 * All three protocols converge on the same Netty-level signal: channel writability.
 * The translation approach is uniform:
 * <ol>
 *   <li>When the <b>target</b> channel becomes unwritable, pause reads on the <b>source</b> channel.</li>
 *   <li>When the <b>target</b> channel becomes writable again, resume reads on the <b>source</b> channel.</li>
 * </ol>
 * This works because:
 * <ul>
 *   <li>For TCP (H1): unwritable = TCP send buffer full → pause source</li>
 *   <li>For HTTP/2: Netty's Http2FlowController marks the channel unwritable when the
 *       flow control window is exhausted → pause source</li>
 *   <li>For QUIC (H3): QUIC's flow control surfaces as channel writability → pause source</li>
 * </ul>
 *
 * <p>Thread safety: All methods are stateless and safe for concurrent use.
 * The caller must ensure they are invoked on the correct EventLoop.</p>
 */
public final class FlowControlTranslator {

    private FlowControlTranslator() {
    }

    /**
     * Propagates backpressure from a target (sink) channel to a source channel.
     *
     * <p>When the target channel becomes unwritable (its outbound buffer exceeds
     * the high water mark), this method disables autoRead on the source channel,
     * causing TCP/QUIC to apply backpressure to the sender. When the target becomes
     * writable again, autoRead is re-enabled.</p>
     *
     * <p>This is the fundamental building block for all flow control translation:
     * <ul>
     *   <li>H2→H1: H1 backend unwritable → pause H2 frontend reads</li>
     *   <li>H1→H2: H2 backend unwritable → pause H1 frontend reads</li>
     *   <li>H2→H3: H3 backend unwritable → pause H2 frontend reads</li>
     *   <li>H3→H2: H2 backend unwritable → pause H3 frontend reads</li>
     *   <li>H1→H3: H3 backend unwritable → pause H1 frontend reads</li>
     *   <li>H3→H1: H1 backend unwritable → pause H3 frontend reads</li>
     * </ul>
     *
     * @param source the channel to control reads on (frontend/inbound)
     * @param target the channel whose writability determines the flow control state
     */
    public static void propagateBackpressure(Channel source, Channel target) {
        if (source == null || target == null) {
            return;
        }
        if (!source.isActive() || !target.isActive()) {
            return;
        }
        source.config().setAutoRead(target.isWritable());
    }

    /**
     * Applies proactive backpressure after writing data to a target channel.
     *
     * <p>Called after each write to the target channel. If the target has become
     * unwritable, immediately pauses reads on the source channel. This is more
     * responsive than waiting for channelWritabilityChanged -- it catches the
     * exact write that tips the buffer over the high water mark.</p>
     *
     * @param source the channel to pause reads on
     * @param target the channel that was just written to
     */
    public static void checkAfterWrite(Channel source, Channel target) {
        if (source == null || target == null) {
            return;
        }
        if (!target.isWritable()) {
            source.config().setAutoRead(false);
        }
    }

    /**
     * Checks aggregate backpressure across multiple target channels.
     *
     * <p>Used for HTTP/2 multiplexed connections where a single frontend connection
     * fans out to multiple backend connections. The frontend should only read when
     * ALL backend channels are writable. If any backend is unwritable, pausing
     * frontend reads prevents data from accumulating in the unwritable backend's
     * outbound buffer.</p>
     *
     * @param source  the frontend channel to control
     * @param targets the backend channels to check
     * @return {@code true} if all targets are writable and reading should continue
     */
    public static boolean checkAggregateBackpressure(Channel source, Iterable<Channel> targets) {
        if (source == null) {
            return false;
        }
        boolean allWritable = true;
        for (Channel target : targets) {
            if (target != null) {
                // Inactive channels are treated as NOT writable to prevent
                // buffering data for a dead channel
                if (!target.isActive() || !target.isWritable()) {
                    allWritable = false;
                    break;
                }
            }
        }
        source.config().setAutoRead(allWritable);
        return allWritable;
    }

    /**
     * Unconditionally pauses reads on the given channel.
     *
     * <p>Used when the proxy detects a condition that requires the source to stop
     * sending data (e.g., body size limit exceeded, stream reset).</p>
     *
     * @param channel the channel to pause
     */
    public static void pauseReads(Channel channel) {
        if (channel != null && channel.isActive()) {
            channel.config().setAutoRead(false);
        }
    }

    /**
     * Unconditionally resumes reads on the given channel.
     *
     * @param channel the channel to resume
     */
    public static void resumeReads(Channel channel) {
        if (channel != null && channel.isActive()) {
            channel.config().setAutoRead(true);
        }
    }

    /**
     * Returns whether the given channel can accept more writes without
     * exceeding its outbound buffer high water mark.
     *
     * @param channel the channel to check
     * @return {@code true} if the channel is writable
     */
    public static boolean isWritable(Channel channel) {
        return channel != null && channel.isWritable();
    }

    /**
     * Configures the write buffer water marks on a channel for optimal
     * flow control behavior in a proxy context.
     *
     * <p>The high water mark determines when the channel becomes unwritable
     * (triggering backpressure). The low water mark determines when it becomes
     * writable again (releasing backpressure). Setting these correctly prevents
     * both excessive buffering (OOM) and excessive flow control toggling (latency).</p>
     *
     * @param channel       the channel to configure
     * @param lowWaterMark  bytes: channel becomes writable when buffer drops below this
     * @param highWaterMark bytes: channel becomes unwritable when buffer exceeds this
     */
    public static void configureWaterMarks(Channel channel, int lowWaterMark, int highWaterMark) {
        if (channel == null) {
            return;
        }
        ChannelConfig config = channel.config();
        // High water mark MUST be set FIRST. If low is set first and the new low
        // value exceeds the current high, Netty throws IllegalArgumentException.
        // Setting high first guarantees high >= low at every intermediate state.
        config.setWriteBufferHighWaterMark(highWaterMark);
        config.setWriteBufferLowWaterMark(lowWaterMark);
    }
}
