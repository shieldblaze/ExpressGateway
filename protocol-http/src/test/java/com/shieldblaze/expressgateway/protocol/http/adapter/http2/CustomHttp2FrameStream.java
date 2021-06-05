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

import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Stream;

final class CustomHttp2FrameStream implements Http2FrameStream {

    private final int id;

    CustomHttp2FrameStream(int id) {
        this.id = id;
    }

    @Override
    public int id() {
        return id;
    }

    @Override
    public Http2Stream.State state() {
        return Http2Stream.State.OPEN;
    }

    @Override
    public String toString() {
        return "CustomHttp2FrameStream{" +
                "id=" + id +
                '}';
    }
}
