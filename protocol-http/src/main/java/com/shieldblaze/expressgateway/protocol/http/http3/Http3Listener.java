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

import com.shieldblaze.expressgateway.protocol.http.ConnectionPool;
import com.shieldblaze.expressgateway.protocol.quic.QuicListener;
import io.netty.channel.Channel;
import io.netty.channel.ChannelInitializer;
import io.netty.handler.codec.http3.Http3ServerConnectionHandler;
import io.netty.handler.codec.quic.QuicSslContext;

import java.util.concurrent.atomic.AtomicReference;

/**
 * HTTP/3 Listener extending the generic {@link QuicListener} with HTTP/3 framing.
 *
 * <p>Installs {@link Http3ServerConnectionHandler} on each new QUIC connection,
 * which manages HTTP/3 control streams (RFC 9114 Section 6.2), QPACK encoder/decoder
 * streams (RFC 9204), and request stream demultiplexing.</p>
 *
 * <p>A single {@link ConnectionPool} is lazily created on the first QUIC connection
 * and shared across all connections and streams. This pool manages backend TCP
 * connections, enabling efficient reuse across HTTP/3 request streams.</p>
 *
 * <p>All QUIC transport setup (UDP binding, QUIC codec, SO_REUSEPORT, transport params)
 * is handled by the parent {@link QuicListener} class.</p>
 */
public class Http3Listener extends QuicListener {

    /**
     * Create a new Http3Listener with the given QUIC TLS context.
     *
     * @param quicSslContext the pre-built QUIC SSL context with H3 ALPN token
     */
    public Http3Listener(QuicSslContext quicSslContext) {
        super(quicSslContext, new Http3ChannelInitializerFactory());
    }

    /**
     * Factory that creates Http3ServerConnectionHandler per QUIC connection,
     * sharing a single ConnectionPool across all connections.
     *
     * <p>The ConnectionPool is lazily initialized on the first create() call
     * (when the L4LoadBalancer is available) using AtomicReference CAS for
     * thread-safe initialization without synchronization overhead.</p>
     */
    private static final class Http3ChannelInitializerFactory
            implements com.shieldblaze.expressgateway.protocol.quic.QuicChannelInitializerFactory {

        private final AtomicReference<ConnectionPool> poolRef = new AtomicReference<>();

        @Override
        public io.netty.channel.ChannelHandler create(
                com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer loadBalancer) {
            // Lazily create the shared ConnectionPool on first QUIC connection.
            // AtomicReference CAS ensures exactly one pool is created even under
            // concurrent connection establishment from multiple EventLoop threads.
            ConnectionPool pool = poolRef.get();
            if (pool == null) {
                ConnectionPool newPool = new ConnectionPool(
                        loadBalancer.configurationContext().httpConfiguration());
                if (poolRef.compareAndSet(null, newPool)) {
                    pool = newPool;
                } else {
                    // Another thread won the race -- close our duplicate and use theirs.
                    newPool.closeAll();
                    pool = poolRef.get();
                }
            }

            final ConnectionPool sharedPool = pool;

            return new Http3ServerConnectionHandler(
                    new ChannelInitializer<Channel>() {
                        @Override
                        protected void initChannel(Channel streamCh) {
                            streamCh.pipeline().addLast(
                                    new Http3ServerHandler(loadBalancer, streamCh, sharedPool)
                            );
                        }
                    }
            );
        }
    }
}
