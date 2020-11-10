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
package com.shieldblaze.expressgateway.core.server.http;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.pool.Connection;
import com.shieldblaze.expressgateway.core.loadbalancer.l7.http.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceResponse;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2TranslatedLastHttpContent;
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentSkipListMap;

public final class UpstreamHandler extends ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    /**
     * Long: Stream Hash
     * Connection: {@link Connection} Instance
     */
    private final Map<Long, Connection> connectionMap = new ConcurrentSkipListMap<>();

    private final HTTPBalance httpBalance;
    private final Bootstrapper bootstrapper;

    public UpstreamHandler(HTTPLoadBalancer httpLoadBalancer) {
        this.httpBalance = httpLoadBalancer.getL7Balance();
        bootstrapper = new Bootstrapper(httpLoadBalancer, httpLoadBalancer.getEventLoopFactory().getChildGroup(), httpLoadBalancer.getByteBufAllocator());
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest) {
            HttpRequest request = (HttpRequest) msg;
            long streamHash = Long.parseLong(request.headers().get(Headers.STREAM_HASH));

            InetSocketAddress upstreamAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            HTTPBalanceRequest balanceRequest = new HTTPBalanceRequest(upstreamAddress, request.headers());
            HTTPBalanceResponse balanceResponse = httpBalance.getResponse(balanceRequest);
            Backend backend = balanceResponse.getBackend();

            HTTPConnection connection = (HTTPConnection) backend.lease();
            if (connection == null) {
                connection = bootstrapper.newInit(backend, ctx.channel());
                backend.addConnection(connection);
            } else {
                connection.setUpstreamChannel(ctx.channel());
                if (!connection.isHTTP2()) {
                    connection.lease();
                }
            }

            connectionMap.put(streamHash, connection);

            // Modify Request Headers
            onHeadersRead(request.headers(), streamHash, balanceResponse.getBackend().getCluster().getHostname(), upstreamAddress);

            // Write the request to Backend
            connection.writeAndFlush(msg);
        } else if (msg instanceof Http2TranslatedHttpContent) {
            Http2TranslatedHttpContent httpContent = (Http2TranslatedHttpContent) msg;
            connectionMap.get(httpContent.streamId()).writeAndFlush(msg);
        }
    }

    private void onHeadersRead(HttpHeaders headers, long streamHash, String host, InetSocketAddress upstreamAddress) {
        headers.remove(HttpHeaderNames.UPGRADE);
        headers.set(HttpHeaderNames.HOST, host);
        headers.set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");
        headers.add(Headers.X_FORWARDED_FOR, upstreamAddress.getAddress().getHostAddress());
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpResponse) {
            HttpResponse response = (HttpResponse) msg;
            response.headers().set(HttpHeaderNames.SERVER, "ShieldBlaze ExpressGateway");
        } else if (msg instanceof DefaultHttp2TranslatedLastHttpContent) {
            DefaultHttp2TranslatedLastHttpContent httpContent = (DefaultHttp2TranslatedLastHttpContent) msg;
            connectionMap.remove(httpContent.streamId());
        }
        super.write(ctx, msg, promise);
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
