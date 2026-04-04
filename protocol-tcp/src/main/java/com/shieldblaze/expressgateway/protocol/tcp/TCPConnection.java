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
import com.shieldblaze.expressgateway.backend.Node;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import lombok.extern.log4j.Log4j2;

/**
 * Downstream {@link Connection} for TCP Protocol
 */
@Log4j2
final class TCPConnection extends Connection {

    private Channel clientChannel;

    TCPConnection(Node node) {
        super(node);
    }

    /**
     * HIGH-13: Set the client (upstream/frontend) channel so it can be closed on backend connect failure.
     */
    void clientChannel(Channel clientChannel) {
        this.clientChannel = clientChannel;
    }

    /**
     * Process Backlog
     *
     * @param channelFuture If the {@link ChannelFuture} is successful, write the backlog to the {@link Channel}
     */
    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            writeBacklog();
        } else {
            // HIGH-13: On backend connect failure, close the client channel to signal error
            log.error("Backend connect failed for node {}: {}",
                    node().socketAddress(), channelFuture.cause() != null ? channelFuture.cause().getMessage() : "unknown");
            clearBacklog();
            if (clientChannel != null && clientChannel.isActive()) {
                clientChannel.close();
            }
        }
    }
}
