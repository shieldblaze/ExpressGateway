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
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpgradeProperty;
import com.shieldblaze.expressgateway.protocol.http.websocket.WebSocketUpstreamHandler;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.UnsupportedMessageTypeException;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpContentCompressor;
import io.netty.handler.codec.http.HttpContentDecompressor;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshaker;
import io.netty.handler.codec.http.websocketx.WebSocketServerHandshakerFactory;
import io.netty.util.ReferenceCountUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URI;
import java.nio.charset.StandardCharsets;

import static io.netty.handler.codec.http.HttpHeaderNames.CONNECTION;
import static io.netty.handler.codec.http.HttpHeaderNames.CONTENT_LENGTH;
import static io.netty.handler.codec.http.HttpHeaderNames.UPGRADE;
import static io.netty.handler.codec.http.HttpResponseStatus.BAD_GATEWAY;
import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;

public class Http11ServerInboundHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(Http11ServerInboundHandler.class);

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final boolean isTLSConnection;

    private HttpConnection httpConnection;

    public Http11ServerInboundHandler(HTTPLoadBalancer httpLoadBalancer, boolean isTLSConnection) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.bootstrapper = new Bootstrapper(httpLoadBalancer);
        this.isTLSConnection = isTLSConnection;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof HttpRequest request) {
            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            Cluster cluster = httpLoadBalancer.cluster(request.headers().getAsString(HttpHeaderNames.HOST));

            // If `Cluster` is `null` then no `Cluster` was found for that Hostname.
            // Throw error back to client, `BAD_GATEWAY`.
            if (cluster == null) {
                ByteBuf responseMessage = ctx.alloc().buffer();
                responseMessage.writeCharSequence(BAD_GATEWAY.reasonPhrase(), StandardCharsets.UTF_8);

                FullHttpResponse fullHttpResponse = new DefaultFullHttpResponse(HTTP_1_1, BAD_GATEWAY, responseMessage);
                fullHttpResponse.headers().set(CONTENT_LENGTH, responseMessage.readableBytes());

                ctx.writeAndFlush(fullHttpResponse).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // If there is no connection then establish one and use it.
            if (httpConnection == null) {
                Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, request.headers())).node();

                if (node.connectionFull()) {
                    HttpResponse httpResponse = new DefaultFullHttpResponse(HTTP_1_1, BAD_GATEWAY);
                    ctx.writeAndFlush(httpResponse).addListener(ChannelFutureListener.CLOSE);
                    return;
                }

                httpConnection = bootstrapper.create(node, ctx.channel());
                node.addConnection(httpConnection);
            }

            // If Upgrade is triggered then don't process this request any further.
            WebSocketUpgradeProperty webSocketProperty = validateWebSocketRequest(ctx, request);
            if (webSocketProperty != null) {
                ctx.pipeline().remove(HttpContentCompressor.class);
                ctx.pipeline().remove(HttpContentDecompressor.class);
                ctx.pipeline().replace(this, "ws", new WebSocketUpstreamHandler(httpConnection.node(), httpLoadBalancer, webSocketProperty));
                return;
            }

            // Modify Request Headers
            // Set supported 'ACCEPT_ENCODING' headers
            request.headers().set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate");

            // Add 'X-Forwarded-For' Header
            request.headers().add(Headers.X_FORWARDED_FOR, socketAddress.getAddress().getHostAddress());

            // Add 'X-Forwarded-Proto' Header
            request.headers().add(Headers.X_FORWARDED_PROTO, isTLSConnection ? "https" : "http");

            // Write the request to Backend
            httpConnection.writeAndFlush(request);
        } else if (msg instanceof HttpContent) {
            httpConnection.writeAndFlush(msg);
        } else {
            ReferenceCountUtil.release(msg);
            throw new UnsupportedMessageTypeException(msg);
        }
    }

    /**
     * Handles HTTP Protocol Upgrades to WebSocket
     *
     * @param ctx         {@linkplain ChannelHandlerContext} associated with this channel
     * @param httpRequest This {@linkplain HttpRequest}
     * @return Returns {@code true} when an upgrade has happened else {@code false}
     */
    private WebSocketUpgradeProperty validateWebSocketRequest(ChannelHandlerContext ctx, HttpRequest httpRequest) {
        HttpHeaders headers = httpRequest.headers();
        String connection = headers.get(CONNECTION);
        String upgrade = headers.get(UPGRADE);

        if (connection == null || upgrade == null) {
            return null;
        }

        // If 'Connection:Upgrade' and 'Upgrade:WebSocket' then begin WebSocket Upgrade Process.
        if (headers.get(CONNECTION).equalsIgnoreCase("Upgrade") && headers.get(UPGRADE).equalsIgnoreCase("WebSocket")) {

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
        } else {
            return null;
        }
    }

    private String webSocketURL(HttpRequest req) {
        String url = req.headers().get(HttpHeaderNames.HOST) + req.uri();

        // If TLS for Client is enabled then use `wss`.
        if (httpLoadBalancer.configurationContext().tlsClientConfiguration() != null) {
            return "wss://" + url;
        }

        return "ws://" + url;
    }

    @Override
    public void channelInactive(ChannelHandlerContext ctx) {
        close();
    }

    @SuppressWarnings("StatementWithEmptyBody")
    @Override
    public void exceptionCaught(ChannelHandlerContext ctx, Throwable cause) {
        try {
            if (cause instanceof IOException) {
                // Swallow this harmless IOException
            } else {
                logger.error("Caught error, Closing connections", cause);
            }
        } finally {
            close();
        }
    }

    @Override
    public void close() {
        if (httpConnection != null) {
            httpConnection.close();
            httpConnection = null;
        }
    }
}
