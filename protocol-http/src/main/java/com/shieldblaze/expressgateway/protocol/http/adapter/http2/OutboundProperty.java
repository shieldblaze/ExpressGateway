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
package com.shieldblaze.expressgateway.protocol.http.adapter.http2;

import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.Http2FrameStream;

final class OutboundProperty {
    private final long nonce;
    private final Http2FrameStream stream;
    private boolean isInitialRead;
    private final HttpVersion httpVersion;

    OutboundProperty(long nonce, Http2FrameStream stream, HttpVersion httpVersion) {
        this.nonce = nonce;
        this.stream = stream;
        this.httpVersion = httpVersion;
    }

    long nonce() {
        return nonce;
    }

    Http2FrameStream stream() {
        return stream;
    }

    boolean initialRead() {
        return isInitialRead;
    }

    void fireInitialRead() {
        isInitialRead = true;
    }

    HttpVersion httpVersion() {
        return httpVersion;
    }
}
