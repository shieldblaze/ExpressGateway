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
package com.shieldblaze.expressgateway.protocol.quic;

import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import io.netty.channel.ChannelHandler;

/**
 * Factory for creating a {@link ChannelHandler} to install on each new QUIC connection.
 *
 * <p>This allows different application protocols to plug into the QUIC listener.
 * For example, HTTP/3 passes a factory that installs {@code Http3ServerConnectionHandler},
 * while other QUIC-based protocols can provide their own handler.</p>
 */
@FunctionalInterface
public interface QuicChannelInitializerFactory {

    /**
     * Create a {@link ChannelHandler} to install on a new {@link io.netty.handler.codec.quic.QuicChannel}.
     *
     * @param loadBalancer the load balancer providing cluster and event loop resources
     * @return the channel handler to install on the QuicChannel pipeline
     */
    ChannelHandler create(L4LoadBalancer loadBalancer);
}
