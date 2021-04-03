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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectOpenHashMap;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

public final class UpstreamHandler extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    /**
     * Long: Request ID
     * HTTPConnection: {@link HTTPConnection} Instance
     */
    private final Long2ObjectMap<HTTPConnection> connectionMap = new Long2ObjectOpenHashMap<>();

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final boolean isTLSConnection;

    public UpstreamHandler(HTTPLoadBalancer httpLoadBalancer, boolean isTLSConnection) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.bootstrapper = new Bootstrapper(httpLoadBalancer, httpLoadBalancer.eventLoopFactory().childGroup(), httpLoadBalancer.byteBufAllocator());
        this.isTLSConnection = isTLSConnection;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            Cluster cluster = httpLoadBalancer.cluster(request.headers().getAsString(HttpHeaderNames.HOST));

            // If `Cluster` is `null` then no `Cluster` was found for that Hostname.
            // Throw error back to client, `BAD_GATEWAY`.
            if (cluster == null) {
                ctx.writeAndFlush(HTTPResponses.BAD_GATEWAY_502).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, request.headers())).node();

            /*
             * We'll try to lease an available connection. If available, we'll get
             * HTTPConnection Instance else we'll get 'null'.
             *
             * If we don't get any available connection, we'll create a new
             * HTTPConnection.
             */
            HTTPConnection connection = validateConnection((HTTPConnection) node.tryLease());

            if (connection == null) {
                connection = bootstrapper.newInit(node, ctx.channel());
                node.addConnection(connection);
            } else {
                // Set this as a new Upstream Channel
                connection.upstreamChannel(ctx.channel());
            }

            // If connection is HTTP/2 then we'll release it back to the pool
            // because we can do multiplexing on HTTP/2.
            if (connection.isHTTP2()) {
                connection.release();
            }

            // Map Id with Connection
            long id = ((HttpFrame) msg).id();
            connectionMap.put(id, connection);

            // Add request Id in outstanding list and increment total number of requests.
            connection.addOutstandingRequest(id);
            connection.incrementTotalRequests();

            // Modify Request Headers
            onHeadersRead(request.headers(), socketAddress);

            // Write the request to Backend
            connection.writeAndFlush(request);
        } else if (msg instanceof HttpFrame) {
            HttpFrame httpFrame = (HttpFrame) msg;
            connectionMap.get(httpFrame.id()).writeAndFlush(msg);
        }
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof CustomLastHttpContent) {
            long id = ((CustomLastHttpContent) msg).id();
            HTTPConnection connection = connectionMap.remove(id); // Remove mapping of finished Request.
            connection.finishedOutstandingRequest(id);
        }
        super.write(ctx, msg, promise);
    }

    private void onHeadersRead(HttpHeaders headers, InetSocketAddress upstreamAddress) {
        /*
         * Remove 'Upgrade' header because we don't support
         * any other protocol than HTTP/1.X and HTTP/2
         */
        headers.remove(HttpHeaderNames.UPGRADE);

        // Set supported 'ACCEPT_ENCODING' headers
        headers.set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");

        // Add 'X-Forwarded-For' Header
        headers.add(Headers.X_FORWARDED_FOR, upstreamAddress.getAddress().getHostAddress());

        // Add 'X-Forwarded-Proto' Header
        headers.add(Headers.X_FORWARDED_PROTO, isTLSConnection ? "HTTPS" : "HTTP");
    }

    private HTTPConnection validateConnection(HTTPConnection httpConnection) {
        if (httpConnection == null) {
            return null;
        } else if (httpConnection.isHTTP2() && httpConnection.hasReachedMaximumCapacity()) {
            httpConnection.close();
            return null;
        } else {
            return httpConnection;
        }
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
