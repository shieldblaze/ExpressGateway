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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceResponse;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
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

            InetSocketAddress upstreamAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            HTTPBalanceRequest balanceRequest = new HTTPBalanceRequest(upstreamAddress, request.headers());
            HTTPBalanceResponse balanceResponse = (HTTPBalanceResponse) httpLoadBalancer.cluster().nextNode(balanceRequest);
            Node node = balanceResponse.node();

            HTTPConnection connection = (HTTPConnection) node.tryLease();
            if (connection == null) {
                connection = bootstrapper.newInit(node, ctx.channel());
                node.addConnection(connection);
            } else {
                connection.upstreamChannel(ctx.channel());
                if (!connection.isHTTP2()) {
                    connection.lease();
                }
            }

            // Map Id with Connection
            connectionMap.put(((HttpFrame) msg).id(), connection);

            // Modify Request Headers
            onHeadersRead(request.headers(), upstreamAddress);

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
        headers.add(Headers.X_FORWARDED_FOR, upstreamAddress.getAddress().getHostAddress());
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
