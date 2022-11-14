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

final class StreamPropertyMap {

    private final Int2ObjectMap<StreamProperty> k1Map = new Int2ObjectArrayMap<>();
    private final Int2ObjectMap<StreamProperty> k2Map = new Int2ObjectArrayMap<>();

    void put(int proxyId, int clientId, StreamProperty v) {
        k1Map.put(proxyId, v);
        k2Map.put(clientId, v);
    }

    void remove(int proxyId, int clientId) {
        k1Map.remove(proxyId);
        k2Map.remove(clientId);
    }

    StreamProperty getClientFromProxyID(int k1) {
        return get(k1, -1);
    }

    StreamProperty getProxyFromClientID(int k2) {
        return get(-1, k2);
    }

    private StreamProperty get(int k1, int k2) {
        if (k1Map.containsKey(k1) && k2Map.containsKey(k2)) {
            if (k1Map.get(k1).equals(k2Map.get(k2))) {
                return k1Map.get(k1);
            } else {
                throw new IllegalArgumentException("Failed to find Mapping");
            }
        } else if (k1Map.containsKey(k1)) {
            return k1Map.get(k1);
        } else return k2Map.getOrDefault(k2, null);
    }

    @Override
    public String toString() {
        return "StreamPropertyMap{" +
                "k1Map=" + k1Map +
                ", k2Map=" + k2Map +
                '}';
    }

    record StreamProperty(String acceptEncoding, Http2FrameStream clientFrameStream, Http2FrameStream proxyFrameStream) {
        // Simple record
    }
}
