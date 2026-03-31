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
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.protocol.quic.QuicBootstrapper;
import com.shieldblaze.expressgateway.protocol.quic.QuicConnectionPool;
import io.netty.buffer.ByteBufAllocator;
import io.netty.channel.Channel;
import io.netty.channel.EventLoopGroup;

/**
 * Creates backend HTTP/3 connections to downstream servers.
 *
 * <p>Delegates to {@link QuicBootstrapper} for QUIC transport setup with "h3"
 * as the ALPN application protocol (RFC 9114 Section 3.1).</p>
 */
public final class Http3Bootstrapper {

    private final QuicBootstrapper<Http3Connection> quicBootstrapper;

    public Http3Bootstrapper(ConfigurationContext configurationContext,
                             EventLoopGroup eventLoopGroup,
                             ByteBufAllocator byteBufAllocator) {
        this.quicBootstrapper = new QuicBootstrapper<>(
                configurationContext, eventLoopGroup, byteBufAllocator, "h3");
    }

    /**
     * Create a new HTTP/3 backend connection to the given Node.
     *
     * <p>The {@code frontendChannel} parameter is intentionally retained for API symmetry
     * with the HTTP/1.1+2 {@code Bootstrapper.create(Node, Channel, ConnectionPool)} and
     * to support future features that may need the frontend channel reference (e.g.,
     * backpressure propagation from backend QUIC streams to the frontend).</p>
     *
     * @param node             the backend node to connect to
     * @param frontendChannel  the client-facing channel (reserved for future use)
     * @param pool             the connection pool to register the new connection in
     * @return the new {@link Http3Connection} (may still be handshaking)
     */
    @SuppressWarnings("unused") // frontendChannel retained for API symmetry with Bootstrapper.create()
    Http3Connection create(Node node, Channel frontendChannel, QuicConnectionPool<Http3Connection> pool) {
        return quicBootstrapper.connect(node, pool, () -> new Http3Connection(node));
    }
}
