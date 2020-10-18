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
import com.shieldblaze.expressgateway.core.server.http.HTTPResponses;
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

import java.net.InetSocketAddress;

public class UpstreamHandler extends Http2ChannelDuplexHandler {

    private HTTPBalance httpBalance;

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

    private void onHeaderRead(ChannelHandlerContext ctx, Http2HeadersFrame http2HeadersFrame) throws Http2Exception {
        Http2Headers headers = http2HeadersFrame.headers();

        HTTPBalanceResponse httpBalanceResponse = httpBalance.getResponse(new HTTPBalanceRequest((InetSocketAddress) ctx.channel().remoteAddress(),
                getHeaders(http2HeadersFrame)));
        if (httpBalanceResponse == null) {
            sendErrorResponse(ctx, HTTPResponses.BAD_GATEWAY, http2HeadersFrame.padding());
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
        DefaultHttpHeaders defaultHttpHeaders = new DefaultHttpHeaders(false);
        HttpConversionUtil.addHttp2ToHttpHeaders(http2HeadersFrame.stream().id(), headers, defaultHttpHeaders, HttpVersion.HTTP_1_1, false, true);
        return defaultHttpHeaders;
    }

    private void sendErrorResponse(ChannelHandlerContext ctx, FullHttpResponse _fullHttpResponse, int padding) {
        FullHttpResponse fullHttpResponse = _fullHttpResponse.retainedDuplicate();
        fullHttpResponse.headers().remove(HttpHeaderNames.CONNECTION);

        Http2Headers http2Headers = HttpConversionUtil.toHttp2Headers(fullHttpResponse, false);
        DefaultHttp2HeadersFrame defaultHttp2HeadersFrame = new DefaultHttp2HeadersFrame(http2Headers, true, padding);
        ctx.channel().writeAndFlush(defaultHttp2HeadersFrame);
    }
}
