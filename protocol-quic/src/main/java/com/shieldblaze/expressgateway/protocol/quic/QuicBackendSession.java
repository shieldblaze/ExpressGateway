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

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import io.netty.channel.ChannelFuture;

/**
 * Connection abstraction for a QUIC proxy backend session.
 *
 * <p>Represents a UDP channel connected to a backend Node for raw QUIC
 * datagram forwarding. Extends {@link Connection} to participate in the
 * standard Node connection tracking and backlog draining lifecycle.</p>
 */
public final class QuicBackendSession extends Connection {

    public QuicBackendSession(Node node) {
        super(node);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            writeBacklog();
        } else {
            clearBacklog();
        }
    }
}
