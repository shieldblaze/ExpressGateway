/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.handler.ssl.ApplicationProtocolNames;

final class HTTPConnection extends Connection {

    private boolean isHTTP2;
    private DownstreamHandler downstreamHandler;

    HTTPConnection(long timeout) {
        super(timeout);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            if (channelFuture.channel().pipeline().get(ALPNHandler.class) != null) {
                channelFuture.channel().pipeline().get(ALPNHandler.class).protocol().whenCompleteAsync((protocol, throwable) -> {

                    // If throwable is null then task is completed successfully without any error.
                    if (throwable == null) {

                        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                            isHTTP2 = true;
                        }

                        writeBacklog(channelFuture);
                    } else {
                        clearBacklog(throwable);
                    }
                }, channelFuture.channel().eventLoop());
            } else {
                writeBacklog(channelFuture);
            }
        } else {
            clearBacklog(channelFuture.cause());
        }
    }

    boolean isHTTP2() {
        return isHTTP2;
    }

    void downstreamHandler(DownstreamHandler downstreamHandler) {
        this.downstreamHandler = downstreamHandler;
    }

    void upstreamChannel(Channel channel) {
        downstreamHandler.channel(channel);
    }

    @Override
    public String toString() {
        return "HTTPConnection{" +
                "isHTTP2=" + isHTTP2 +
                ", Connection=" + super.toString() +
                '}';
    }
}
