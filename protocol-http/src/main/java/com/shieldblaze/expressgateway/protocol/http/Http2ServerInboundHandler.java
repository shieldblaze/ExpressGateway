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
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelInboundHandlerAdapter;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2SettingsFrame;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.io.IOException;
import java.net.InetSocketAddress;

import static io.netty.handler.codec.http.HttpResponseStatus.BAD_GATEWAY;
import static io.netty.handler.codec.http.HttpResponseStatus.INTERNAL_SERVER_ERROR;

public final class Http2ServerInboundHandler extends ChannelInboundHandlerAdapter implements Closeable {

    private static final Logger logger = LogManager.getLogger(Http2ServerInboundHandler.class);

    private final HTTPLoadBalancer httpLoadBalancer;
    private final Bootstrapper bootstrapper;
    private final boolean isTLSConnection;

    private ChannelHandlerContext ctx;
    private HttpConnection httpConnection;
    private Http2SettingsFrame http2SettingsFrame;

    public Http2ServerInboundHandler(HTTPLoadBalancer httpLoadBalancer, boolean isTLSConnection) {
        this.httpLoadBalancer = httpLoadBalancer;
        this.bootstrapper = new Bootstrapper(httpLoadBalancer);
        this.isTLSConnection = isTLSConnection;
    }

    @Override
    public void channelActive(ChannelHandlerContext ctx) {
        this.ctx = ctx;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2SettingsFrame settingsFrame) {
            http2SettingsFrame = settingsFrame;
        } else if (msg instanceof Http2HeadersFrame headersFrame) {
            InetSocketAddress socketAddress = (InetSocketAddress) ctx.channel().remoteAddress();
            Cluster cluster = httpLoadBalancer.cluster(String.valueOf(headersFrame.headers().authority()));

            // If `Cluster` is `null` then no `Cluster` was found for that Hostname.
            // Throw error back to client, `BAD_GATEWAY`.
            if (cluster == null) {
                Http2Headers http2Headers = new DefaultHttp2Headers();
                http2Headers.status(INTERNAL_SERVER_ERROR.codeAsText());

                Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers);
                responseHeadersFrame.stream(headersFrame.stream());

                ctx.writeAndFlush(responseHeadersFrame).addListener(ChannelFutureListener.CLOSE);
                return;
            }

            // If there is no connection then establish one and use it.
            if (httpConnection == null) {
                Node node = cluster.nextNode(new HTTPBalanceRequest(socketAddress, headersFrame.headers())).node();
                if (node.connectionFull()) {
                    Http2Headers http2Headers = new DefaultHttp2Headers();
                    http2Headers.status(BAD_GATEWAY.toString());

                    Http2HeadersFrame responseHeadersFrame = new DefaultHttp2HeadersFrame(http2Headers);
                    responseHeadersFrame.stream(headersFrame.stream());

                    ctx.writeAndFlush(responseHeadersFrame).addListener(ChannelFutureListener.CLOSE);
                    return;
                }

                httpConnection = bootstrapper.create(node, ctx.channel(), http2SettingsFrame.settings());
                node.addConnection(httpConnection);
            }

            // Add 'X-Forwarded-For' Header
            headersFrame.headers().add(Headers.X_FORWARDED_FOR, socketAddress.getAddress().getHostAddress());

            // Add 'X-Forwarded-Proto' Header
            headersFrame.headers().add(Headers.X_FORWARDED_PROTO, isTLSConnection ? "https" : "http");
        }

        // If Http2Settings is not null then we have pending Http2Settings to write.
        if (http2SettingsFrame != null) {

            // If HttpConnection is null then connection is not ready. We will write the Http2Settings
            // once the HttpConnection becomes active.
            if (httpConnection == null) {
                return;
            } else {
                httpConnection.writeAndFlush(http2SettingsFrame);
                http2SettingsFrame = null;
            }
        }

        if (httpConnection == null) {
            System.err.println("HTTPConnection is null: " + msg);
            ReferenceCountedUtil.silentRelease(msg);
        } else {
            httpConnection.writeAndFlush(msg);
        }
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
    public synchronized void close() {
        if (ctx != null) {
            ctx.channel().close();
            ctx = null;
        }

        if (httpConnection != null) {
            httpConnection.close();
            httpConnection = null;
        }
    }
}
