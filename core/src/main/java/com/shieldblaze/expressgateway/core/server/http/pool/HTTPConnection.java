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
package com.shieldblaze.expressgateway.core.server.http.pool;

import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandler;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelFutureListener;
import io.netty.handler.ssl.ApplicationProtocolNames;

import java.util.function.BiConsumer;

public final class HTTPConnection extends Connection {

    private boolean ALPNHandlerPresent;
    private Protocol protocol;

    @Override
    public void setChannelFuture(ChannelFuture channelFuture) {
        if (this.channelFuture == null) {
            this.channelFuture = channelFuture;

            // Add Listener to write all pending backlog data.
            this.channelFuture.addListener((ChannelFutureListener) future -> {

                if (future.isSuccess()) {

                    // If ALPNHandlerPresent is 'true' then we will add listener and wait for ALPN negotiation to finish.
                    // If listener call backs and throwable is 'null' then we'll write backlog else we'll release backlog.
                    if (ALPNHandlerPresent) {
                        ALPNHandler alpnHandler = future.channel().pipeline().get(ALPNHandler.class);
                        alpnHandler.getALPNProtocol().whenCompleteAsync(new BiConsumer<String, Throwable>() {
                            @Override
                            public void accept(String protocolAsString, Throwable throwable) {
                                if (protocolAsString.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                                    protocol = Protocol.HTTP_2;
                                } else {
                                    protocol = Protocol.HTTP_1_1;
                                }
                                releaseBacklog(throwable == null);
                            }
                        }, GlobalExecutors.INSTANCE.getExecutorService());
                    } else {
                        releaseBacklog(true);
                    }
                } else {
                    releaseBacklog(false);
                }
            });
        } else {
            throw new IllegalArgumentException("ChannelFuture is already set");
        }
    }

    void setALPNHandlerPresent() {
        this.ALPNHandlerPresent = true;
    }

    public Protocol getProtocol() {
        return protocol;
    }

    public enum Protocol {
        HTTP_2,
        HTTP_1_1
    }
}
