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
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

public final class UpstreamHandler extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    /**
     * Long: Request ID
     * Connection: {@link Connection} Instance
     */
    private final Map<Long, Connection> connectionMap = new ConcurrentHashMap<>();

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;

    public UpstreamHandler(HTTPLoadBalancer httpLoadBalancer) {
        this.httpLoadBalancer = httpLoadBalancer;
        bootstrapper = new Bootstrapper(httpLoadBalancer, httpLoadBalancer.eventLoopFactory().childGroup(), httpLoadBalancer.byteBufAllocator());
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;

            // If Host Header is not present then return `BAD_REQUEST` error.
            if (!request.headers().contains(HttpHeaderNames.HOST)) {
                ctx.writeAndFlush(HTTPResponses.BAD_REQUEST_400).addListener(ChannelFutureListener.CLOSE);
                return;
            }

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
            HTTPConnection connection = (HTTPConnection) node.tryLease();
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
            connectionMap.put(((HttpFrame) msg).id(), connection);

            // Modify Request Headers
            onHeadersRead(request.headers(), socketAddress);

            // Write the request to Backend
            connection.writeAndFlush(request);
        } else if (msg instanceof HttpFrame) {
            HttpFrame httpFrame = (HttpFrame) msg;
            connectionMap.get(httpFrame.id()).writeAndFlush(msg);
        }
    }

    private void onHeadersRead(HttpHeaders headers, InetSocketAddress upstreamAddress) {
        headers.remove(HttpHeaderNames.UPGRADE);
        headers.set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");
        headers.add("x-forwarded-for", upstreamAddress.getAddress().getHostAddress());
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
