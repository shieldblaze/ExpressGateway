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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Connection;
import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.protocol.http.alpn.ALPNHandler;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.handler.ssl.ApplicationProtocolNames;
import it.unimi.dsi.fastutil.longs.LongArrayList;
import it.unimi.dsi.fastutil.longs.LongList;

import java.util.concurrent.atomic.AtomicInteger;

final class HTTPConnection extends Connection {

    private final AtomicInteger totalRequests = new AtomicInteger();
    private final LongList outstandingRequests = new LongArrayList();

    /**
     * Set to {@code true} if this connection is established on top of HTTP/2 (h2)
     */
    private boolean isHTTP2;
    private DownstreamHandler downstreamHandler;

    HTTPConnection(Node node) {
        super(node);
    }

    @Override
    protected void processBacklog(ChannelFuture channelFuture) {
        ALPNHandler alpnHandler = channelFuture.channel().pipeline().get(ALPNHandler.class);

        // If operation was successful then we'll check if ALPNHandler is available or not.
        // If ALPNHandler is available (not null) then we'll see if ALPN has negotiated HTTP/2 or not.
        // We'll then write the backlog or clear the backlog.
        if (channelFuture.isSuccess()) {
            if (alpnHandler != null) {
                alpnHandler.protocol().whenCompleteAsync((protocol, throwable) -> {

                    // If throwable is 'null' then task is completed successfully without any error.
                    if (throwable == null) {

                        if (protocol.equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                            isHTTP2 = true;

                            /*
                             * Since HTTP/2 supports Multiplexing,
                             * we'll release this connection back to pool.
                             */
                            release();
                        }

                        writeBacklog();
                    } else {
                        clearBacklog();
                    }
                }, channelFuture.channel().eventLoop());
            } else {
                writeBacklog();
            }
        } else {
            clearBacklog();
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

    void addOutstandingRequest(long id) {
        outstandingRequests.add(id);
    }

    boolean isRequestOutstanding(long id) {
        return outstandingRequests.contains(id);
    }

    void finishedOutstandingRequest(long id) {
        outstandingRequests.rem(id);
    }

    void incrementTotalRequests() {
        totalRequests.incrementAndGet();
    }

    boolean hasReachedMaximumCapacity() {
        return totalRequests.get() > Integer.MAX_VALUE - 100_000;
    }

    @Override
    public String toString() {
        return "HTTPConnection{" + "isHTTP2=" + isHTTP2 + ", Connection=" + super.toString() + '}';
    }
}
