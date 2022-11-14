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
package com.shieldblaze.expressgateway.protocol.http;

import io.netty.handler.codec.http2.Http2FrameStream;
import it.unimi.dsi.fastutil.ints.Int2ObjectArrayMap;
import it.unimi.dsi.fastutil.ints.Int2ObjectMap;
import it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap;

final class StreamPropertyMap {

    private final Int2ObjectMap<StreamProperty> map = new Int2ObjectOpenHashMap<>();

    void put(int streamId, StreamProperty streamProperty) {
        map.put(streamId, streamProperty);
    }

    StreamProperty remove(Http2FrameStream frameStream) {
        return map.remove(frameStream.id());
    }

    StreamProperty remove(int streamId) {
        return map.remove(streamId);
    }

    StreamProperty get(Http2FrameStream streamFrame) {
        return map.get(streamFrame.id());
    }

    StreamProperty get(int streamId) {
        return map.get(streamId);
    }

    @Override
    public String toString() {
        return "StreamPropertyMap{map=" + map + '}';
    }

    record StreamProperty(String acceptEncoding, Http2FrameStream clientFrameStream, Http2FrameStream proxyFrameStream) {
        // Simple record
    }
}
