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

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.exceptions.TooManyConnectionsException;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Response;
import com.shieldblaze.expressgateway.core.handlers.ConnectionTimeoutHandler;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.channel.ChannelOption;
import io.netty.channel.socket.ChannelInputShutdownEvent;
import io.netty.channel.socket.SocketChannel;
import io.netty.util.ReferenceCountUtil;
import lombok.extern.log4j.Log4j2;

import java.io.IOException;
import java.net.InetSocketAddress;

@Log4j2
final class UpstreamHandler extends ChannelInboundHandlerAdapter {

    private final L4LoadBalancer l4LoadBalancer;
    private final Bootstrapper bootstrapper;
    private final ConnectionStatistics statistics = new ConnectionStatistics();
    private TCPConnection tcpConnection;
    private volatile boolean closed; // LOW-25: Guard against double-close
    private volatile boolean draining; // Connection draining flag for graceful shutdown

    UpstreamHandler(L4LoadBalancer l4LoadBalancer) {
        this.l4LoadBalancer = l4LoadBalancer;
        bootstrapper = new Bootstrapper(l4LoadBalancer);
    }

    /**
     * Returns the connection statistics for this upstream session.
     */
    ConnectionStatistics statistics() {
        return statistics;
    }

    /**
     * Mark this connection as draining. No new data will be accepted from the client,
     * but existing buffered data will be flushed to the backend before closing.
     */
    void startDraining() {
        this.draining = true;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        try {
            // Get the next node from the cluster to handle this request
            L4Response response = (L4Response) l4LoadBalancer.defaultCluster().nextNode(new L4Request((InetSocketAddress) ctx.channel().remoteAddress()));

            // Close the connection since we have no node available to handle this request
            if (response == L4Response.NO_NODE) {
                ctx.close();
                return;
            }

            // Create a new TCPConnection and add it to the node
            tcpConnection = bootstrapper.newInit(response.node(), ctx.channel());
            try {
                response.node().addConnection(tcpConnection);
            } catch (TooManyConnectionsException ex) {
                log.warn("Node {} rejected connection: too many connections", response.node().socketAddress());
                tcpConnection = null;
                ctx.close();
                return;
            }
        } catch (Exception ex) {
            tcpConnection = null;
            ctx.close();
            throw ex;
        }
    }

    // BP-TCP-01: Backpressure -- when the client channel's writability changes,
    // toggle autoRead on the backend channel. If the client's outbound buffer is
    // full (DownstreamHandler is writing backend data to client faster than the
    // client can drain), pause reads on the backend channel to stop the inflow.
    // When the client buffer drains and becomes writable again, resume backend reads.
    @Override
    public void channelWritabilityChanged(ChannelHandlerContext ctx) throws Exception {
        if (tcpConnection != null) {
            Channel backendChannel = tcpConnection.channel();
            if (backendChannel != null) {
                boolean writable = ctx.channel().isWritable();
                backendChannel.config().setAutoRead(writable);
                if (!writable) {
                    statistics.recordBackpressurePause();
                }
            }
        }
        super.channelWritabilityChanged(ctx);
    }

    // HIGH-10: Use write() instead of writeAndFlush() for flush coalescing
    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        if (closed || tcpConnection == null || draining) {
            ReferenceCountUtil.release(msg);
            return;
        }

        // Track bytes for statistics
        if (msg instanceof ByteBuf buf) {
            statistics.recordBytesRead(buf.readableBytes());
        }

        tcpConnection.write(msg);

        // BP-TCP-01: Proactive backpressure -- after writing to the backend channel,
        // if its outbound buffer is full, immediately pause reads on the client
        // channel. This prevents unbounded accumulation in the backend's write queue
        // when the backend is slower than the client. Guard against null channel
        // during connection initialization (writes go to backlog queue in that state).
        Channel backendChannel = tcpConnection.channel();
        if (backendChannel != null && !backendChannel.isWritable()) {
            ctx.channel().config().setAutoRead(false);
            statistics.recordBackpressurePause();
        }
    }

    // HIGH-10: Flush all buffered writes when the read batch is complete
    @Override
    public void channelReadComplete(ChannelHandlerContext ctx) {
        if (tcpConnection != null) {
            tcpConnection.flush();
        }
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        if (closed) {
            return; // LOW-25: Prevent double-close
        }

        if (log.isInfoEnabled()) {
            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            if (tcpConnection == null || tcpConnection.socketAddress() == null) {
                log.info("Closing Upstream {}", socketAddress.getAddress().getHostAddress() + ':' + socketAddress.getPort());
            } else {
                log.info("Closing Upstream {} and Downstream {} Channel",
                        socketAddress.getAddress().getHostAddress() + ':' + socketAddress.getPort(),
                        tcpConnection.socketAddress().getAddress().getHostAddress() + ':' + tcpConnection.socketAddress().getPort());
            }
        }

        closeAll(ctx);
    }

    @Override
    public void userEventTriggered(ChannelHandlerContext ctx, Object evt) throws Exception {
        // If ConnectionTimeoutHandler event is caught then close upstream and downstream channels.
        if (evt instanceof ConnectionTimeoutHandler.State) {
            closeAll(ctx);
        } else if (evt instanceof ChannelInputShutdownEvent) {
            // RFC 9293 Section 3.6: Client sent FIN (half-close). Relay to backend.
            // Flush any pending data before shutting down output to ensure the backend
            // receives all data that was written before the FIN arrived.
            if (tcpConnection != null) {
                tcpConnection.flush();
                Channel backendChannel = tcpConnection.channel();
                if (backendChannel != null && backendChannel.isActive()) {
                    // Flush then shutdownOutput via a listener to ensure ordering
                    backendChannel.writeAndFlush(Unpooled.EMPTY_BUFFER).addListener(future ->
                        ((SocketChannel) backendChannel).shutdownOutput()
                    );
                }
            }
        } else {
            super.userEventTriggered(ctx, evt);
        }
    }

    private void closeAll(ChannelHandlerContext ctx) {
        if (closed) {
            return; // LOW-25: Prevent double-close
        }
        closed = true;

        // Close the upstream channel if it is active
        if (ctx.channel().isActive()) {
            ctx.channel().close();
        }

        // Close the downstream connection. Handle both INITIALIZED (still connecting)
        // and CONNECTED_AND_ACTIVE states. A connection in INITIALIZED state has a
        // pending ChannelFuture; closing it cancels the connect attempt and releases
        // any queued backlog messages.
        if (tcpConnection != null) {
            Connection.State state = tcpConnection.state();
            if (state == Connection.State.CONNECTED_AND_ACTIVE || state == Connection.State.INITIALIZED) {
                tcpConnection.close();
            }
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        log.error("Caught Error at Upstream Handler", cause);

        // MED-13: RST propagation -- when the client sends a TCP RST (detected as an
        // IOException containing "reset"), propagate it to the backend by setting
        // SO_LINGER(0) on the backend channel before closing. This causes the kernel
        // to send RST instead of FIN, faithfully relaying the reset signal.
        if (cause instanceof IOException && tcpConnection != null) {
            String msg = cause.getMessage();
            if (msg != null && msg.contains("reset")) {
                Channel backendChannel = tcpConnection.channel();
                if (backendChannel != null && backendChannel.isActive()) {
                    backendChannel.config().setOption(ChannelOption.SO_LINGER, 0);
                }
            }
        }
        // Close on all exceptions to prevent leaking connections on non-IO errors
        // (e.g., OutOfMemoryError, codec exceptions, handler bugs).
        closeAll(ctx);
    }
}
