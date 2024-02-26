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
package com.shieldblaze.expressgateway.protocol.http.alpn;

import io.netty.channel.ChannelHandler;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * Builder for {@link ALPNHandler}
 */
public final class ALPNHandlerBuilder {

    private final Map<String, ChannelHandler> HTTP1ChannelHandlers = new LinkedHashMap<>();
    private final Map<String, ChannelHandler> HTTP2ChannelHandlers = new LinkedHashMap<>();

    private ALPNHandlerBuilder() {
        // Prevent outside initialization
    }

    public static ALPNHandlerBuilder newBuilder() {
        return new ALPNHandlerBuilder();
    }

    public ALPNHandlerBuilder withHTTP1ChannelHandler(ChannelHandler channelHandler) {
        HTTP1ChannelHandlers.put("ChannelHandler#" + HTTP1ChannelHandlers.size(), channelHandler);
        return this;
    }

    public ALPNHandlerBuilder withHTTP1ChannelHandler(String name, ChannelHandler channelHandler) {
        HTTP1ChannelHandlers.put(name, channelHandler);
        return this;
    }

    public ALPNHandlerBuilder withHTTP2ChannelHandler(ChannelHandler channelHandler) {
        HTTP2ChannelHandlers.put("ChannelHandler#" + HTTP2ChannelHandlers.size(), channelHandler);
        return this;
    }

    public ALPNHandlerBuilder withHTTP2ChannelHandler(String name, ChannelHandler channelHandler) {
        HTTP2ChannelHandlers.put(name, channelHandler);
        return this;
    }

    /**
     * Build {@linkplain ALPNHandler} Instance
     *
     * @return {@linkplain ALPNHandler Instance}
     */
    public ALPNHandler build() {
        if (!(!HTTP1ChannelHandlers.isEmpty() || !HTTP2ChannelHandlers.isEmpty())) {
            throw new IllegalArgumentException("There must be at least one Handler for both HTTP/2 and HTTP/1.1");
        }
        return new ALPNHandler(HTTP1ChannelHandlers, HTTP2ChannelHandlers);
    }
}
