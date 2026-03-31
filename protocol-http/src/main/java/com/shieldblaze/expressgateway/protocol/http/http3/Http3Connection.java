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
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.metrics.StandardEdgeNetworkMetricRecorder;
import com.shieldblaze.expressgateway.protocol.quic.QuicConnection;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.quic.QuicChannel;

/**
 * HTTP/3 backend connection extending {@link QuicConnection} with HTTP/3-specific behavior.
 *
 * <p>Adds HTTP/3 GOAWAY semantics (error code 0x100 per RFC 9114) and per-backend
 * active stream metrics recording on top of the generic QUIC connection.</p>
 */
public final class Http3Connection extends QuicConnection {

    /**
     * Timestamp (nanoTime) when the current request was sent to the backend.
     */
    volatile long requestStartNanos;

    @NonNull
    Http3Connection(Node node) {
        super(node);
    }

    /**
     * Returns {@code true} -- this is always an HTTP/3 connection.
     */
    boolean isHttp3() {
        return true;
    }

    @Override
    public int incrementActiveStreams() {
        int count = super.incrementActiveStreams();
        recordActiveStreamMetric(count);
        return count;
    }

    @Override
    public int decrementActiveStreams() {
        int count = super.decrementActiveStreams();
        recordActiveStreamMetric(count);
        return count;
    }

    /**
     * Send an HTTP/3 GOAWAY frame to the backend for graceful shutdown per
     * RFC 9114 Section 5.2. Uses H3_NO_ERROR (0x100) application error code.
     */
    @Override
    public ChannelFuture sendGoaway() {
        QuicChannel qc = quicChannel();
        if (qc == null || !qc.isActive()) {
            return null;
        }
        return qc.close(true, 0x100, Unpooled.EMPTY_BUFFER);
    }

    /**
     * Update the per-backend active H3 streams gauge in the global metric recorder.
     */
    private void recordActiveStreamMetric(int count) {
        Node n = node();
        if (n != null) {
            // Use "h3:" prefix to distinguish HTTP/3 streams from HTTP/2 in metrics
            StandardEdgeNetworkMetricRecorder.INSTANCE.recordActiveH2Streams(
                    "h3:" + n.socketAddress().toString(), count);
        }
    }

    @Override
    public String toString() {
        return "Http3Connection{" + "activeStreams=" + activeStreams()
                + ", idleSinceNanos=" + idleSinceNanos()
                + ", Connection=" + super.toString() + '}';
    }
}
