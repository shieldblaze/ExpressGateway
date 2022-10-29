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
package com.shieldblaze.expressgateway.protocol.http.adapter.http1;

import com.shieldblaze.expressgateway.protocol.http.NonceWrapped;
import com.shieldblaze.expressgateway.protocol.http.adapter.http2.HTTP2InboundAdapter;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;

/**
 * {@linkplain HTTPOutboundAdapter} handles incoming HTTP/1.x responses
 * and makes them compatible with {@linkplain HTTP2InboundAdapter}.
 */
public final class HTTPOutboundAdapter extends ChannelDuplexHandler {

    private long nonce;

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
        NonceWrapped<?> nonceWrapped = (NonceWrapped<?>) msg;
        nonce = nonceWrapped.nonce();
        ctx.write(nonceWrapped.get(), promise);
    }

    @Override
    public void channelRead(ChannelHandlerContext ctx, Object msg) {
        ctx.fireChannelRead(new NonceWrapped<>(nonce, msg));
    }
}
