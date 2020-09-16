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
package com.shieldblaze.expressgateway.healthcheck.l7;

import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import io.netty.bootstrap.Bootstrap;
import io.netty.channel.Channel;
import io.netty.channel.ChannelFuture;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.HttpConversionUtil;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.SslContext;
import io.netty.util.concurrent.Promise;

import java.net.InetSocketAddress;
import java.net.URL;
import java.util.concurrent.TimeUnit;

public final class HTTP extends HealthCheck {

    private final Bootstrap bootstrap;
    private final SslContext sslContext;
    final URL url;
    final int timeout;

    public HTTP(Bootstrap bootstrap, SslContext sslContext, URL url, int timeout) {
        super(new InetSocketAddress(url.getHost(), url.getPort()), timeout);
        this.bootstrap = bootstrap;
        this.sslContext = sslContext;
        this.url = url;
        this.timeout = timeout;
    }

    @Override
    public void check() {
        Channel channel = null;
        try {

            bootstrap.handler(new Initializer(sslContext, this));
            ChannelFuture channelFuture = bootstrap.connect(socketAddress);
            channelFuture.await(5, TimeUnit.SECONDS);
            channel = channelFuture.channel();

            if (!channelFuture.isSuccess()) {
                markFailure();
                return;
            }

            Promise<String> alpnPromise = channel.pipeline().get(Initializer.ALPNHandler.class).getPromise();
            alpnPromise.await(timeout, TimeUnit.SECONDS);
            if (!alpnPromise.isSuccess()) {
                markFailure();
                return;
            }

            DefaultFullHttpRequest fullHttpRequest = new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, url.getPath());
            fullHttpRequest.headers()
                    .add(HttpHeaderNames.HOST, url.getHost())
                    .add(HttpHeaderNames.USER_AGENT, "ShieldBlaze ExpressGateway HealthCheck Client");

            if (alpnPromise.get().equalsIgnoreCase(ApplicationProtocolNames.HTTP_2)) {
                fullHttpRequest.headers().add(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
            }

            channel.writeAndFlush(fullHttpRequest).sync();
            Promise<Boolean> promise = channel.pipeline().get(Handler.class).getPromise();
            promise.await(timeout, TimeUnit.SECONDS);

            if (promise.isSuccess() && promise.get()) {
                markSuccess();
            } else {
                markFailure();
            }

        } catch (Exception ex) {
            ex.printStackTrace();
            markFailure();
        } finally {
            if (channel != null) {
                channel.close();
            }
        }
    }

    @Override
    protected void markSuccess() {
        super.markSuccess();
    }

    @Override
    protected void markFailure() {
        super.markFailure();
    }
}
