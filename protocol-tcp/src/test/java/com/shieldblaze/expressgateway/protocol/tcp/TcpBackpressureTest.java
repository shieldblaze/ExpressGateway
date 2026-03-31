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

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.Channel;
import io.netty.channel.ChannelConfig;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.DefaultChannelConfig;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.verify;
import static org.mockito.Mockito.when;

/**
 * BP-TCP-01: Tests for bidirectional TCP backpressure.
 *
 * <p>Verifies that when one side of the proxy becomes unwritable (outbound buffer full),
 * the other side's autoRead is disabled (pausing inbound reads), and that when writability
 * is restored, autoRead is re-enabled.</p>
 */
final class TcpBackpressureTest {

    private Cluster cluster;
    private Node node;

    @BeforeEach
    void setUp() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", 9999))
                .build();
    }

    // -------------------------------------------------------------------
    // DownstreamHandler tests (lives on backend pipeline, writes to client)
    // -------------------------------------------------------------------

    /**
     * When the backend channel becomes unwritable (its outbound buffer is full because
     * UpstreamHandler is writing client data faster than the backend can drain),
     * the client (upstream) channel's autoRead must be set to false.
     */
    @Test
    void downstreamHandler_backendUnwritable_pausesClientReads() throws Exception {
        // Mock the upstream (client) channel and its config
        Channel upstreamChannel = mock(Channel.class);
        ChannelConfig upstreamConfig = mock(ChannelConfig.class);
        when(upstreamChannel.config()).thenReturn(upstreamConfig);
        when(upstreamChannel.remoteAddress()).thenReturn(new InetSocketAddress("10.0.0.1", 12345));

        DownstreamHandler handler = new DownstreamHandler(upstreamChannel, node);

        // Mock the backend channel (ctx.channel()) as unwritable
        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel backendChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(backendChannel);
        when(backendChannel.isWritable()).thenReturn(false);

        // Fire writability changed
        handler.channelWritabilityChanged(ctx);

        // Verify: upstream autoRead set to false (backend is unwritable)
        verify(upstreamConfig).setAutoRead(false);
    }

    /**
     * When the backend channel becomes writable again (outbound buffer drained),
     * the client (upstream) channel's autoRead must be re-enabled.
     */
    @Test
    void downstreamHandler_backendWritable_resumesClientReads() throws Exception {
        Channel upstreamChannel = mock(Channel.class);
        ChannelConfig upstreamConfig = mock(ChannelConfig.class);
        when(upstreamChannel.config()).thenReturn(upstreamConfig);
        when(upstreamChannel.remoteAddress()).thenReturn(new InetSocketAddress("10.0.0.1", 12345));

        DownstreamHandler handler = new DownstreamHandler(upstreamChannel, node);

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel backendChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(backendChannel);
        when(backendChannel.isWritable()).thenReturn(true);

        handler.channelWritabilityChanged(ctx);

        verify(upstreamConfig).setAutoRead(true);
    }

    /**
     * Proactive backpressure in channelRead: after writing to the client channel,
     * if the client is unwritable, backend autoRead must be disabled immediately
     * (don't wait for channelWritabilityChanged).
     */
    @Test
    void downstreamHandler_channelRead_pausesBackendWhenClientUnwritable() throws Exception {
        Channel upstreamChannel = mock(Channel.class);
        ChannelConfig upstreamConfig = mock(ChannelConfig.class);
        when(upstreamChannel.config()).thenReturn(upstreamConfig);
        when(upstreamChannel.remoteAddress()).thenReturn(new InetSocketAddress("10.0.0.1", 12345));
        when(upstreamChannel.isActive()).thenReturn(true);
        when(upstreamChannel.isWritable()).thenReturn(false);
        // write() returns a mock ChannelFuture (voidPromise)
        when(upstreamChannel.voidPromise()).thenReturn(mock(io.netty.channel.ChannelPromise.class));
        when(upstreamChannel.write(org.mockito.ArgumentMatchers.any(), org.mockito.ArgumentMatchers.any()))
                .thenReturn(mock(io.netty.channel.ChannelFuture.class));

        DownstreamHandler handler = new DownstreamHandler(upstreamChannel, node);

        // Mock the backend channel (ctx.channel())
        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel backendChannel = mock(Channel.class);
        ChannelConfig backendConfig = mock(ChannelConfig.class);
        when(ctx.channel()).thenReturn(backendChannel);
        when(backendChannel.config()).thenReturn(backendConfig);

        // Simulate reading data from backend
        ByteBuf msg = Unpooled.wrappedBuffer(new byte[]{1, 2, 3});
        handler.channelRead(ctx, msg);

        // Verify: backend autoRead set to false because client is unwritable
        verify(backendConfig).setAutoRead(false);

        msg.release();
    }

    /**
     * When the client channel is writable after a write, backend autoRead
     * should NOT be changed (it remains whatever it was).
     */
    @Test
    void downstreamHandler_channelRead_noChangeWhenClientWritable() throws Exception {
        Channel upstreamChannel = mock(Channel.class);
        ChannelConfig upstreamConfig = mock(ChannelConfig.class);
        when(upstreamChannel.config()).thenReturn(upstreamConfig);
        when(upstreamChannel.remoteAddress()).thenReturn(new InetSocketAddress("10.0.0.1", 12345));
        when(upstreamChannel.isActive()).thenReturn(true);
        when(upstreamChannel.isWritable()).thenReturn(true); // Client is writable
        when(upstreamChannel.voidPromise()).thenReturn(mock(io.netty.channel.ChannelPromise.class));
        when(upstreamChannel.write(org.mockito.ArgumentMatchers.any(), org.mockito.ArgumentMatchers.any()))
                .thenReturn(mock(io.netty.channel.ChannelFuture.class));

        DownstreamHandler handler = new DownstreamHandler(upstreamChannel, node);

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel backendChannel = mock(Channel.class);
        ChannelConfig backendConfig = mock(ChannelConfig.class);
        when(ctx.channel()).thenReturn(backendChannel);
        when(backendChannel.config()).thenReturn(backendConfig);

        ByteBuf msg = Unpooled.wrappedBuffer(new byte[]{1, 2, 3});
        handler.channelRead(ctx, msg);

        // Verify: backend autoRead was NOT touched (no setAutoRead call)
        verify(backendConfig, org.mockito.Mockito.never()).setAutoRead(false);

        msg.release();
    }

    // -------------------------------------------------------------------
    // UpstreamHandler tests (lives on client pipeline, writes to backend)
    // -------------------------------------------------------------------

    /**
     * When the client channel becomes unwritable (its outbound buffer is full because
     * DownstreamHandler is writing backend data faster than the client can drain),
     * the backend channel's autoRead must be set to false.
     */
    @Test
    void upstreamHandler_clientUnwritable_pausesBackendReads() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerWithConnection();

        // Get the mock backend channel from the TCPConnection
        TCPConnection conn = getTcpConnection(handler);
        Channel backendChannel = conn.channel();
        ChannelConfig backendConfig = mock(ChannelConfig.class);
        when(backendChannel.config()).thenReturn(backendConfig);

        // Mock ctx.channel() (client channel) as unwritable
        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.isWritable()).thenReturn(false);

        handler.channelWritabilityChanged(ctx);

        // Verify: backend autoRead set to false (client is unwritable)
        verify(backendConfig).setAutoRead(false);
    }

    /**
     * When the client channel becomes writable again, the backend channel's
     * autoRead must be re-enabled.
     */
    @Test
    void upstreamHandler_clientWritable_resumesBackendReads() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerWithConnection();

        TCPConnection conn = getTcpConnection(handler);
        Channel backendChannel = conn.channel();
        ChannelConfig backendConfig = mock(ChannelConfig.class);
        when(backendChannel.config()).thenReturn(backendConfig);

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.isWritable()).thenReturn(true);

        handler.channelWritabilityChanged(ctx);

        verify(backendConfig).setAutoRead(true);
    }

    /**
     * When tcpConnection is null (backend not yet established),
     * channelWritabilityChanged must not throw.
     */
    @Test
    void upstreamHandler_nullConnection_writabilityChangedDoesNotThrow() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerNoConnection();

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.isWritable()).thenReturn(false);

        // Must not throw NullPointerException
        handler.channelWritabilityChanged(ctx);
    }

    /**
     * When tcpConnection.channel() is null (backend connecting, not yet resolved),
     * channelWritabilityChanged must not throw.
     */
    @Test
    void upstreamHandler_nullBackendChannel_writabilityChangedDoesNotThrow() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerWithNullChannel();

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.isWritable()).thenReturn(false);

        // Must not throw NullPointerException
        handler.channelWritabilityChanged(ctx);
    }

    /**
     * Proactive backpressure in channelRead: after writing to the backend channel,
     * if the backend is unwritable, client autoRead must be disabled immediately.
     */
    @Test
    void upstreamHandler_channelRead_pausesClientWhenBackendUnwritable() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerWithConnection();

        TCPConnection conn = getTcpConnection(handler);
        Channel backendChannel = conn.channel();
        when(backendChannel.isWritable()).thenReturn(false);

        // Mock ctx.channel() (client channel)
        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        ChannelConfig clientConfig = mock(ChannelConfig.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.config()).thenReturn(clientConfig);

        // We need the TCPConnection to be in CONNECTED_AND_ACTIVE state
        // so that write() actually calls channel.write() instead of queueing
        setConnectionState(conn, com.shieldblaze.expressgateway.backend.Connection.State.CONNECTED_AND_ACTIVE);

        ByteBuf msg = Unpooled.wrappedBuffer(new byte[]{4, 5, 6});
        when(backendChannel.write(org.mockito.ArgumentMatchers.any()))
                .thenReturn(mock(io.netty.channel.ChannelFuture.class));

        handler.channelRead(ctx, msg);

        // Verify: client autoRead set to false because backend is unwritable
        verify(clientConfig).setAutoRead(false);
    }

    /**
     * When the backend is writable after a write, client autoRead should NOT be changed.
     */
    @Test
    void upstreamHandler_channelRead_noChangeWhenBackendWritable() throws Exception {
        UpstreamHandler handler = createUpstreamHandlerWithConnection();

        TCPConnection conn = getTcpConnection(handler);
        Channel backendChannel = conn.channel();
        when(backendChannel.isWritable()).thenReturn(true); // Backend is writable

        ChannelHandlerContext ctx = mock(ChannelHandlerContext.class);
        Channel clientChannel = mock(Channel.class);
        ChannelConfig clientConfig = mock(ChannelConfig.class);
        when(ctx.channel()).thenReturn(clientChannel);
        when(clientChannel.config()).thenReturn(clientConfig);

        setConnectionState(conn, com.shieldblaze.expressgateway.backend.Connection.State.CONNECTED_AND_ACTIVE);

        ByteBuf msg = Unpooled.wrappedBuffer(new byte[]{4, 5, 6});
        when(backendChannel.write(org.mockito.ArgumentMatchers.any()))
                .thenReturn(mock(io.netty.channel.ChannelFuture.class));

        handler.channelRead(ctx, msg);

        // Verify: client autoRead was NOT touched
        verify(clientConfig, org.mockito.Mockito.never()).setAutoRead(false);
    }

    // -------------------------------------------------------------------
    // Helper methods
    // -------------------------------------------------------------------

    /**
     * Creates a mock L4LoadBalancer that satisfies the Bootstrapper constructor
     * (which calls eventLoopFactory().childGroup() and byteBufAllocator()).
     */
    private com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer mockL4LoadBalancer() {
        com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer lb =
                mock(com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer.class);

        com.shieldblaze.expressgateway.core.factory.EventLoopFactory elf =
                mock(com.shieldblaze.expressgateway.core.factory.EventLoopFactory.class);
        when(elf.childGroup()).thenReturn(mock(io.netty.channel.EventLoopGroup.class));
        when(lb.eventLoopFactory()).thenReturn(elf);
        when(lb.byteBufAllocator()).thenReturn(io.netty.buffer.ByteBufAllocator.DEFAULT);

        return lb;
    }

    /**
     * Creates an UpstreamHandler with a mock L4LoadBalancer and injects a
     * TCPConnection with a mock backend channel via reflection.
     */
    private UpstreamHandler createUpstreamHandlerWithConnection() throws Exception {
        UpstreamHandler handler = new UpstreamHandler(mockL4LoadBalancer());

        // Create a real TCPConnection and inject a mock channel
        TCPConnection conn = new TCPConnection(node);
        Channel mockBackend = mock(Channel.class);
        setField(conn, "channel", mockBackend);

        // Inject the tcpConnection into the handler
        setField(handler, "tcpConnection", conn);

        return handler;
    }

    /**
     * Creates an UpstreamHandler with no tcpConnection (null — simulating
     * pre-connect state).
     */
    private UpstreamHandler createUpstreamHandlerNoConnection() {
        return new UpstreamHandler(mockL4LoadBalancer());
    }

    /**
     * Creates an UpstreamHandler with a TCPConnection whose channel() returns null
     * (simulating the connecting state before the backend channel is resolved).
     */
    private UpstreamHandler createUpstreamHandlerWithNullChannel() throws Exception {
        UpstreamHandler handler = new UpstreamHandler(mockL4LoadBalancer());

        // Create a TCPConnection with no channel set (channel == null)
        TCPConnection conn = new TCPConnection(node);
        setField(handler, "tcpConnection", conn);

        return handler;
    }

    private TCPConnection getTcpConnection(UpstreamHandler handler) throws Exception {
        Field field = UpstreamHandler.class.getDeclaredField("tcpConnection");
        field.setAccessible(true);
        return (TCPConnection) field.get(handler);
    }

    /**
     * Sets a field value via reflection, searching up the class hierarchy.
     */
    private static void setField(Object target, String fieldName, Object value) throws Exception {
        Class<?> clazz = target.getClass();
        while (clazz != null) {
            try {
                Field field = clazz.getDeclaredField(fieldName);
                field.setAccessible(true);
                field.set(target, value);
                return;
            } catch (NoSuchFieldException e) {
                clazz = clazz.getSuperclass();
            }
        }
        throw new NoSuchFieldException(fieldName + " not found in " + target.getClass().getName() + " hierarchy");
    }

    /**
     * Sets the Connection state via reflection (protected field on base class).
     */
    private static void setConnectionState(TCPConnection conn,
                                           com.shieldblaze.expressgateway.backend.Connection.State state) throws Exception {
        setField(conn, "state", state);
    }
}
