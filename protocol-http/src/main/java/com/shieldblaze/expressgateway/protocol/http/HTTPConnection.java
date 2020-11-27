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

import com.shieldblaze.expressgateway.backend.connection.Backlog;
import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.common.utils.ReferenceCounted;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.handler.ssl.ApplicationProtocolNames;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.concurrent.ConcurrentLinkedQueue;

final class HTTPConnection extends Connection {

    private static final Logger logger = LogManager.getLogger(HTTPConnection.class);

    private boolean isHTTP2;
    private DownstreamHandler downstreamHandler;

    HTTPConnection(long timeout) {
        super(timeout);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        if (channelFuture.isSuccess()) {
            channelFuture.channel().pipeline().get(ALPNHandler.class).protocol().whenCompleteAsync((protocol, throwable) -> {
                if (throwable == null) {

                    if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                        isHTTP2 = true;
                    } else {
                        try {
                            lease();
                        } catch (IllegalAccessException e) {
                            logger.error(e);
                        }
                    }

                    ConcurrentLinkedQueue<Backlog> queue = new ConcurrentLinkedQueue<>(backlogQueue); // Make copy of Queue
                    backlogQueue = null; // Make old queue null so no more data is written to it.
                    queue.forEach(backlog -> channelFuture.channel().writeAndFlush(backlog.object(), backlog.channelPromise()));
                    queue.clear(); // Clear the new queue because we're done with it.
                } else {
                    forceCleanBacklog(throwable);
                }
            }, channelFuture.channel().eventLoop());
        } else {
            forceCleanBacklog(channelFuture.cause());
        }
    }

    private void forceCleanBacklog(Throwable throwable) {
        backlogQueue.forEach(backlog -> {
            ReferenceCounted.silentRelease(backlog.object());
            backlog.channelPromise().tryFailure(throwable);
        });
        backlogQueue = null;
    }

    boolean isHTTP2() {
        return isHTTP2;
    }

    void setDownstreamHandler(DownstreamHandler downstreamHandler) {
        this.downstreamHandler = downstreamHandler;
    }

    void setUpstreamChannel(Channel channel) {
        downstreamHandler.setChannel(channel);
    }

    @Override
    public String toString() {
        return "ALPNConnection{" +
                "isHTTP2=" + isHTTP2 +
                ", Connection=" + super.toString() +
                '}';
    }
}
