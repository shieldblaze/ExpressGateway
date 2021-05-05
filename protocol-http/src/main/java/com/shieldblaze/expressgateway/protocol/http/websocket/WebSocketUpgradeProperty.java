/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.websocket;

import io.netty.channel.Channel;

import java.net.InetSocketAddress;
import java.net.URI;

/**
 * This class hold important objects for fulfilling WebSocket Upgrade Request.
 */
public final class WebSocketUpgradeProperty {
    private final InetSocketAddress clientAddress;
    private final URI uri;
    private final String subProtocol;
    private final Channel channel;

    /**
     * Create a new {@link WebSocketUpgradeProperty} Instance
     *
     * @param clientAddress {@link InetSocketAddress} of Client
     * @param uri           HTTP Request URI
     * @param subProtocol   WebSocket SubProtocol
     * @param channel       {@link Channel} of Client
     */
    public WebSocketUpgradeProperty(InetSocketAddress clientAddress, URI uri, String subProtocol, Channel channel) {
        this.clientAddress = clientAddress;
        this.uri = uri;
        this.subProtocol = subProtocol;
        this.channel = channel;
    }

    InetSocketAddress clientAddress() {
        return clientAddress;
    }

    URI uri() {
        return uri;
    }

    String subProtocol() {
        return subProtocol;
    }

    Channel channel() {
        return channel;
    }
}
