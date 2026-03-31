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
package com.shieldblaze.expressgateway.protocol.tcp;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import io.netty.buffer.ByteBuf;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelOption;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

final class DownstreamHandler extends ChannelInboundHandlerAdapter {

    private static final Logger logger = LogManager.getLogger(DownstreamHandler.class);

    private final Channel upstream;
    private final Node node;
    private final InetSocketAddress upstreamAddress;
    private final ConnectionStatistics statistics = new ConnectionStatistics();
    private volatile boolean closed; // LOW-24: Guard against double-close

    DownstreamHandler(Channel upstream, Node node) {
        this.upstream = upstream;
        this.node = node;
        upstreamAddress = (InetSocketAddress) upstream.remoteAddress();
    }

    /**
     * Returns the connection statistics for this downstream session.
     */
    ConnectionStatistics statistics() {
        return statistics;
    }

    // BP-TCP-01: Backpressure -- when the backend channel's writability changes,
    // toggle autoRead on the client (upstream) channel. If the backend's outbound
    // buffer is full (UpstreamHandler is writing client data to backend faster than
    // the backend can drain), pause reads on the client channel to stop the inflow.
    // When the backend buffer drains and becomes writable again, resume client reads.
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        boolean writable = ctx.channel().isWritable();
        upstream.config().setAutoRead(writable);
        if (!writable) {
            statistics.recordBackpressurePause();
        }
        super.channelWritabilityChanged(ctx);
    }

    // HIGH-10: Use write() instead of writeAndFlush() for flush coalescing
    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        // Track bytes for statistics
        if (msg instanceof ByteBuf buf) {
            statistics.recordBytesRead(buf.readableBytes());
        }

        // Check upstream is still active before writing. Using the default promise
        // instead of voidPromise so write failures are propagated and trigger
        // exceptionCaught, rather than being silently swallowed.
        if (upstream.isActive()) {
            upstream.write(msg);
        } else {
            io.netty.util.ReferenceCountUtil.release(msg);
            return;
        }

        // BP-TCP-01: Proactive backpressure -- after writing to the client channel,
        // if its outbound buffer is full, immediately pause reads on the backend
        // channel. This prevents unbounded accumulation in the client's write queue
        // when the client is slower than the backend.
        if (!upstream.isWritable()) {
            ctx.channel().config().setAutoRead(false);
            statistics.recordBackpressurePause();
        }
    }

    // HIGH-10: Flush all buffered data when the read batch is complete
    @Override
    public void channelReadComplete(ChannelHandlerContext ctx) {
        upstream.flush();
    }

    /**
     * HIGH-12: Handle idle timeout events from ConnectionTimeoutHandler.
     * When the backend connection is idle, close both upstream and downstream channels.
     * Also handles RFC 9293 Sec 3.6 half-close relay from backend to client.
     */
    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        if (evt instanceof ConnectionTimeoutHandler.State) {
            logger.info("Backend idle timeout detected for upstream {}, closing connections",
                    upstreamAddress.getAddress().getHostAddress() + ':' + upstreamAddress.getPort());
            closeAll(ctx);
        } else if (evt instanceof io.netty.channel.socket.ChannelInputShutdownEvent) {
            // RFC 9293 Sec 3.6: Backend sent FIN (half-close). Relay to client.
            if (upstream.isActive()) {
                ((io.netty.channel.socket.SocketChannel) upstream).shutdownOutput();
            }
        } else {
            super.userEventTriggered(ctx, evt);
        }
    }

    /**
     * Downstream Channel is closed, so we close the Upstream Channel as well.
     */
    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (closed) {
            return; // LOW-24: Prevent double-close
        }

        if (logger.isInfoEnabled()) {
            logger.info("Closing Upstream {} and Downstream {} Channel",
                    upstreamAddress.getAddress().getHostAddress() + ':' + upstreamAddress.getPort(),
                    node.socketAddress().getAddress().getHostAddress() + ':' + node.socketAddress().getPort());
        }

        closeAll(ctx);
    }

    private void closeAll(ChannelHandlerContext ctx) {
        if (closed) {
            return; // LOW-24: Prevent double-close
        }
        closed = true;
        upstream.close();      // Close Upstream Channel
        ctx.channel().close(); // Close Downstream Channel
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Downstream Handler", cause);

        // MED-13: Propagate RST from backend to client via SO_LINGER(0).
        // Connection reset is detected as an IOException in exceptionCaught,
        // NOT in channelInactive (which fires for both FIN and RST).
        if (cause instanceof java.io.IOException && upstream.isActive()) {
            String msg = cause.getMessage();
            if (msg != null && msg.contains("reset")) {
                try {
                    upstream.config().setOption(ChannelOption.SO_LINGER, 0);
                } catch (Exception e) {
                    logger.debug("Failed to set SO_LINGER(0) for RST propagation", e);
                }
            }
        }
        closeAll(ctx);
    }
}
