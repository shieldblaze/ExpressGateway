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

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.common.utils.Hostname;
import com.shieldblaze.expressgateway.core.server.http.HTTPResponses;
import com.shieldblaze.expressgateway.loadbalance.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.loadbalance.exceptions.NoBackendAvailableException;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalance;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceResponse;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2ChannelDuplexHandler;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Exception;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;

public final class UpstreamHandler extends Http2ChannelDuplexHandler {

    private static final Logger logger = LogManager.getLogger(UpstreamHandler.class);

    private HTTPBalance httpBalance;
    private Cluster cluster;

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
        HTTPBalanceResponse httpBalanceResponse;
        Http2Headers headers = http2HeadersFrame.headers();
        InetSocketAddress localAddress = (InetSocketAddress) ctx.channel().localAddress();
        InetSocketAddress remoteAddress = (InetSocketAddress) ctx.channel().remoteAddress();

        // Check if Authority header is present.
        // If Authority header is present, check if Authority matches Cluster Hostname and local Port.
        if (headers.authority() == null || !Hostname.doesHostAndPortMatch(cluster.getHostname(), localAddress.getPort(), headers.authority().toString())) {
            sendErrorResponse(ctx, HTTPResponses.BAD_REQUEST_400, http2HeadersFrame.padding());
        }

        // Call HTTPBalance and get HTTPBalanceResponse for this request.
        try {
            httpBalanceResponse = httpBalance.getResponse(new HTTPBalanceRequest(remoteAddress, getHeaders(http2HeadersFrame)));
        } catch (BackendNotOnlineException ex) {
            sendErrorResponse(ctx, HTTPResponses.BAD_GATEWAY_502, http2HeadersFrame.padding());
            return;
        } catch (NoBackendAvailableException ex) {
            sendErrorResponse(ctx, HTTPResponses.SERVICE_UNAVAILABLE_503, http2HeadersFrame.padding());
            return;
        }

        Backend backend = httpBalanceResponse.getBackend();
    }

    private void onDataRead(ChannelHandlerContext ctx, Http2DataFrame http2HeadersFrame) {

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
