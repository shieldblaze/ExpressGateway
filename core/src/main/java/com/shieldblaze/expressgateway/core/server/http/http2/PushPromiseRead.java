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

import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2StreamFrame;

public class PushPromiseRead implements Http2StreamFrame {

    private Http2FrameStream stream;
    private final int promisedStreamId;
    private final Http2Headers headers;
    private final int padding;

    public PushPromiseRead(int promisedStreamId, Http2Headers headers, int padding) {
        this.promisedStreamId = promisedStreamId;
        this.headers = headers;
        this.padding = padding;
    }

    @Override
    public Http2StreamFrame stream(Http2FrameStream stream) {
        this.stream = stream;
        return this;
    }

    @Override
    public Http2FrameStream stream() {
        return stream;
    }

    public int getPromisedStreamId() {
        return promisedStreamId;
    }

    public Http2Headers getHeaders() {
        return headers;
    }

    public int getPadding() {
        return padding;
    }

    @Override
    public String name() {
        return "PUSH_PROMISE_READ";
    }

    @Override
    public String toString() {
        return "PushPromiseRead{" +
                "streamId=" + stream.id() +
                ", promisedStreamId=" + promisedStreamId +
                ", headers=" + headers +
                ", padding=" + padding +
                '}';
    }
}
