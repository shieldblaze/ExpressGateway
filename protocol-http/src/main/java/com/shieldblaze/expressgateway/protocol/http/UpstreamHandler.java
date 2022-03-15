/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import com.shieldblaze.expressgateway.common.utils.ReferenceCountedUtil;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTPContentCompressor;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpgradeProperty;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpstreamHandler;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.CustomFullHttpResponse;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshaker;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshakerFactory;
import it.unimi.dsi.fastutil.longs.Long2ObjectMap;
import it.unimi.dsi.fastutil.longs.Long2ObjectOpenHashMap;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.net.URI;

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
        this.bootstrapper = new Bootstrapper(httpLoadBalancer);
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
                ctx.writeAndFlush(HTTPResponses.BAD_GATEWAY_502.retainedDuplicate()).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, request.headers())).node();

            // If Upgrade is triggered, don't process this request any further.
            WebSocketUpgradeProperty webSocketUpgradeProperty = webSocketUpgrader(ctx, request);
            if (webSocketUpgradeProperty != null) {
                ctx.pipeline().remove(HTTPServerValidator.class);
                ctx.pipeline().remove(HTTPContentCompressor.class);
                ctx.pipeline().remove(HttpContentDecompressor.class);
                ctx.pipeline().replace(this, "WebSocketHandler", new WebSocketUpstreamHandler(node, httpLoadBalancer, webSocketUpgradeProperty));
                return;
            }

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
            return;
        } else if (msg instanceof HttpFrame) {
            HTTPConnection httpConnection = connectionMap.get(((HttpFrame) msg).id());
            if (httpConnection != null) {
                httpConnection.writeAndFlush(msg);
                return;
            }
        }
        ReferenceCountedUtil.silentRelease(msg);
    }

    /**
     * Handles HTTP Protocol Upgrades to WebSocket
     *
     * @param ctx         {@linkplain ChannelHandlerContext} associated with this channel
     * @param httpRequest This {@linkplain HttpRequest}
     * @return Returns {@code true} when an upgrade has happened else {@code false}
     */
    private WebSocketUpgradeProperty webSocketUpgrader(ChannelHandlerContext ctx, HttpRequest httpRequest) {

        // If Header does not contain `Connection` or `Upgrade`, return null.
        if (!httpRequest.headers().contains(HttpHeaderNames.CONNECTION) || !httpRequest.headers().contains(HttpHeaderNames.UPGRADE)) {
            return null;
        }

        // If 'Connection:Upgrade' and 'Upgrade:WebSocket' then begin WebSocket Upgrade Process.
        if (httpRequest.headers().get(HttpHeaderNames.CONNECTION).equalsIgnoreCase("Upgrade") &&
                httpRequest.headers().get(HttpHeaderNames.UPGRADE).equalsIgnoreCase("WebSocket")) {

            // Handshake for WebSocket
            String uri = webSocketURL(httpRequest);
            String subProtocol = httpRequest.headers().get(HttpHeaderNames.SEC_WEBSOCKET_PROTOCOL);
            WebSocketServerHandshakerFactory wsFactory = new WebSocketServerHandshakerFactory(uri, subProtocol, true);
            WebSocketServerHandshaker handshaker = wsFactory.newHandshaker(httpRequest);
            if (handshaker == null) {
                WebSocketServerHandshakerFactory.sendUnsupportedVersionResponse(ctx.channel());
            } else {
                handshaker.handshake(ctx.channel(), httpRequest);
            }

            return new WebSocketUpgradeProperty(((InetSocketAddress) ctx.channel().remoteAddress()), URI.create(uri), subProtocol, ctx.channel());
        }

        return null;
    }

    private String webSocketURL(HttpRequest req) {
        String url = req.headers().get(HttpHeaderNames.HOST) + req.uri();

        // If TLS for Client is enabled then use `wss`.
        if (httpLoadBalancer.configurationContext().tlsClientConfiguration() != null) {
            return "wss://" + url;
        }

        return "ws://" + url;
    }

    private HTTPConnection validateConnection(HTTPConnection httpConnection) {
        if (httpConnection == null) {
            return null;
        } else if (httpConnection.isHTTP2() && httpConnection.hasReachedMaximumCapacity()) {
            // If Connection is established over HTTP/2 and we've reached maximum capacity then
            // close the connection and return null.
            httpConnection.close();
            return null;
        } else {
            return httpConnection;
        }
    }

    private void onHeadersRead(HttpHeaders headers, InetSocketAddress upstreamAddress) {
        // Set supported 'ACCEPT_ENCODING' headers
        headers.set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");

        // Add 'X-Forwarded-For' Header
        headers.add(Headers.X_FORWARDED_FOR, upstreamAddress.getAddress().getHostAddress());

        // Add 'X-Forwarded-Proto' Header
        headers.add(Headers.X_FORWARDED_PROTO, isTLSConnection ? "https" : "http");
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof CustomFullHttpResponse || msg instanceof CustomLastHttpContent) {
            long id = ((HttpFrame) msg).id();
            HTTPConnection connection = connectionMap.remove(id); // Remove mapping of finished Request.
            connection.finishedOutstandingRequest(id);
            connection.release();
        }
        super.write(ctx, msg, promise);
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        connectionMap.forEach((id, connection) -> connection.release());
    }

    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        logger.error("Caught Error at Upstream Handler", cause);
    }
}
