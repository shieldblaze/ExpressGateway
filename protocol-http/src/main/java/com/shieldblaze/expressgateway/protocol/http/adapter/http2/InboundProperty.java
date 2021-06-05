/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http2.Http2FrameStream;

final class InboundProperty {

    private final HttpFrame httpFrame;
    private final Http2FrameStream stream;
    private final String acceptEncoding;

    InboundProperty(HttpFrame httpFrame, Http2FrameStream stream, String acceptEncoding) {
        this.httpFrame = httpFrame;
        this.stream = stream;
        this.acceptEncoding = acceptEncoding;
    }

    HttpFrame httpFrame() {
        return httpFrame;
    }

    Http2FrameStream stream() {
        return stream;
    }

    String acceptEncoding() {
        return acceptEncoding;
    }

    @Override
    public String toString() {
        return "InboundProperty{" +
                "httpFrame=(" + httpFrame.id() + ")" +
                ", stream=" + stream +
                ", acceptEncoding='" + acceptEncoding + '\'' +
                '}';
    }
}
