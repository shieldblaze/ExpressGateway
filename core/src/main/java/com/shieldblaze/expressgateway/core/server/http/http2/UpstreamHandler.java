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
package com.shieldblaze.expressgateway.core.server.http.http2;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.connection.Connection;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.common.utils.Hostname;
import com.shieldblaze.expressgateway.core.server.http.DownstreamHandler;
import com.shieldblaze.expressgateway.core.server.http.HTTPResponses;
import com.shieldblaze.expressgateway.core.server.http.alpn.ALPNHandler;
import com.shieldblaze.expressgateway.core.server.http.pool.HTTPClusterConnectionPool;
import com.shieldblaze.expressgateway.core.server.http.pool.HTTPConnection;
import com.shieldblaze.expressgateway.loadbalance.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceResponse;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.SimpleChannelInboundHandler;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2GoAwayFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Error;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.codec.http2.StreamBufferingEncoder;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentSkipListMap;

public final class UpstreamHandler extends Http2ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    /**
     * <p> {@linkplain Integer}: Upstream Stream ID </p>
     * <p> {@linkplain Property}: Property Instance </p>
     */
    private final Map<Integer, Property> propertyMap = new ConcurrentSkipListMap<>();

    private final HTTPBalance httpBalance;
    private final Cluster cluster;
    private final HTTPClusterConnectionPool connectionPool;

    public UpstreamHandler(HTTPBalance httpBalance, Cluster cluster, HTTPClusterConnectionPool connectionPool) {
        this.httpBalance = httpBalance;
        this.cluster = cluster;
        this.connectionPool = connectionPool;
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) throws Exception {
        if (msg instanceof Http2HeadersFrame) {
            onHeaderRead(ctx, (Http2HeadersFrame) msg);
        } else if (msg instanceof Http2DataFrame) {
            onDataRead(ctx, (Http2DataFrame) msg);
        } else if (msg instanceof PushPromiseRead) {
            onPushPromiseRead(ctx, (PushPromiseRead) msg);
        }
    }

    private void onHeaderRead(ChannelHandlerContext ctx, Http2HeadersFrame http2HeadersFrame) throws Exception {
        int streamId = http2HeadersFrame.stream().id();
        int padding = http2HeadersFrame.padding();
        boolean endOfStream = http2HeadersFrame.isEndStream();
        HTTPConnection httpConnection;
        HTTPBalanceResponse httpBalanceResponse;
        Http2Headers headers = http2HeadersFrame.headers();
        InetSocketAddress localAddress = (InetSocketAddress) ctx.channel().localAddress();
        InetSocketAddress remoteAddress = (InetSocketAddress) ctx.channel().remoteAddress();

        // If 'property' is not 'null' then this frame is part of continuation.
        Property property = propertyMap.get(streamId);
        if (property != null) {
            DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(headers, endOfStream, padding);
            defaultHttp2HeadersFrame.stream(property.getDownstreamFrameStream());
            property.getConnection().writeAndFlush(defaultHttp2HeadersFrame);
            return;
        }

        // Check if Authority header is present.
        // If Authority header is present, check if Authority matches Cluster Hostname and local Port.
        if (headers.authority() == null || !Hostname.doesHostAndPortMatch(cluster.getHostname(), localAddress.getPort(), headers.authority().toString())) {
            sendErrorResponse(ctx, HTTPResponses.BAD_REQUEST_400, padding);
        }

        // Call HTTPBalance and get HTTPBalanceResponse for this request.
        try {
            httpBalanceResponse = httpBalance.getResponse(new HTTPBalanceRequest(remoteAddress, getHeaders(http2HeadersFrame)));
            httpConnection = connectionPool.acquireConnection(httpBalanceResponse, new SimpleChannelInboundHandler<Object>() {
                @Override
                protected void channelRead0(ChannelHandlerContext ctx, Object msg) throws Exception {
                    System.out.println(msg);
                }
            });
        } catch (BackendNotOnlineException ex) {
            sendErrorResponse(ctx, HTTPResponses.BAD_GATEWAY_502, padding);
            return;
        } catch (NoBackendAvailableException ex) {
            sendErrorResponse(ctx, HTTPResponses.SERVICE_UNAVAILABLE_503, padding);
            return;
        }

        property = new Property();
        property.setUpstreamFrameStream(http2HeadersFrame.stream());
        property.setDownstreamFrameStream(super.newStream());
        property.setConnection(httpConnection);
        propertyMap.put(streamId, property);

        // If header does not contains 'CONTENT-ENCODING' then the request is not encoded.
        if (!headers.contains(HttpHeaderNames.CONTENT_ENCODING)) {

            // If header contains 'ACCEPT-ENCODING' then we'll store it.
            if (headers.contains(HttpHeaderNames.ACCEPT_ENCODING)) {
                property.setAcceptEncoding(headers.get(HttpHeaderNames.ACCEPT_ENCODING).toString());
            }

            headers.set(HttpHeaderNames.ACCEPT_ENCODING, "br, gzip, deflate"); // Set `ACCEPT-ENCODING` header
        }

//        headers.set("X-Forwarded-For", ((InetSocketAddress) ctx.channel().remoteAddress()).getAddress().getHostAddress()); // Add 'X-Forwarded-For' header
        headers.authority(cluster.getHostname());

        DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(headers, endOfStream, padding);
        defaultHttp2HeadersFrame.stream(property.getDownstreamFrameStream());
        httpConnection.writeAndFlush(defaultHttp2HeadersFrame);
    }

    private void onDataRead(ChannelHandlerContext ctx, Http2DataFrame http2DataFrame) {
        Property property = propertyMap.get(http2DataFrame.stream().id());
        if (property == null) {
            ctx.channel().writeAndFlush(new DefaultHttp2GoAwayFrame(Http2Error.REFUSED_STREAM));
        } else {
            DefaultHttp2DataFrame defaultHttp2DataFrame = new DefaultHttp2DataFrame(http2DataFrame.content(), http2DataFrame.isEndStream(), http2DataFrame.padding());
            defaultHttp2DataFrame.stream(property.getDownstreamFrameStream());
            property.getConnection().writeAndFlush(defaultHttp2DataFrame);
        }
    }

    private void onPushPromiseRead(ChannelHandlerContext ctx, PushPromiseRead pushPromiseRead) {

    }

    private HttpHeaders getHeaders(Http2HeadersFrame http2HeadersFrame) throws Http2Exception {
        Http2Headers headers = http2HeadersFrame.headers();
        DefaultHttpHeaders defaultHttpHeaders = new DefaultHttpHeaders(true);
        HttpConversionUtil.addHttp2ToHttpHeaders(http2HeadersFrame.stream().id(), headers, defaultHttpHeaders, HttpVersion.HTTP_1_1, false, true);
        return defaultHttpHeaders;
    }

    private void sendErrorResponse(ChannelHandlerContext ctx, FullHttpResponse _fullHttpResponse, int padding) {
        FullHttpResponse fullHttpResponse = _fullHttpResponse.retainedDuplicate();
        fullHttpResponse.headers().remove(HttpHeaderNames.CONNECTION);

        Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpResponse, true);
        DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true, padding);
        ctx.channel().writeAndFlush(defaultHttp2HeadersFrame);
    }
}
