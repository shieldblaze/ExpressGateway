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

public final class WebSocketUpgradeProperty {
    private final InetSocketAddress clientAddress;
    private final URI uri;
    private final String subProtocol;
    private final Channel channel;

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
