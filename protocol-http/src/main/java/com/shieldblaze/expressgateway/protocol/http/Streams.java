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

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

final class Streams {
    // PERF-1: Replaced ReadWriteLock + Int2ObjectOpenHashMap with ConcurrentHashMap.
    // The ReadWriteLock added ~20ns per DATA frame on the hot path. All reads (get,
    // getByBackendId) are on the frontend EventLoop; writes (put, remove) come from
    // the backend EventLoop. ConcurrentHashMap provides thread-safe access with lower
    // overhead than ReadWriteLock for this access pattern. The autoboxing cost of
    // Integer keys is negligible: stream IDs < 128 hit Integer cache, and for higher
    // IDs the boxing is per-stream (put/remove), not per-DATA-frame (get).
    private final ConcurrentHashMap<Integer, Stream> map = new ConcurrentHashMap<>();

    /**
     * Reverse map: backend stream ID -> Stream entry. Needed because with per-stream
     * connection pooling, frontend and backend stream IDs are decoupled (auto-increment).
     * The outbound path stores by frontend ID; the inbound response path looks up by
     * backend ID. This map bridges the two.
     */
    private final ConcurrentHashMap<Integer, Stream> reverseMap = new ConcurrentHashMap<>();

    void put(int streamId, Stream stream) {
        map.put(streamId, stream);
    }

    /**
     * Register a mapping from the backend stream ID to the Stream entry.
     * Called BEFORE writeAndFlush() -- the stream ID is assigned by {@code newStream()},
     * not by the write. Registering before write prevents a race where a fast backend
     * responds before the write listener fires (PB-01).
     */
    void putByBackendId(int backendStreamId, Stream stream) {
        reverseMap.put(backendStreamId, stream);
    }

    Stream remove(int streamId) {
        return map.remove(streamId);
    }

    /**
     * Remove a stream entry by its backend stream ID (from the reverse map).
     */
    Stream removeByBackendId(int backendStreamId) {
        return reverseMap.remove(backendStreamId);
    }

    Stream get(int streamId) {
        return map.get(streamId);
    }

    /**
     * Look up a stream entry by backend stream ID.
     */
    Stream getByBackendId(int backendStreamId) {
        return reverseMap.get(backendStreamId);
    }

    /**
     * Remove all streams with ID greater than {@code lastStreamId}.
     * This is used when handling GOAWAY frames to clean up streams
     * that will not be processed by the remote peer.
     */
    void removeStreamsAbove(int lastStreamId) {
        map.keySet().removeIf(id -> id > lastStreamId);
        reverseMap.keySet().removeIf(id -> id > lastStreamId);
    }

    /**
     * MED-04: Remove all stream mappings where the backend (proxy) stream ID is above
     * the given lastStreamId. Used for GOAWAY processing where the lastStreamId is
     * in the backend stream ID domain, not the frontend domain.
     *
     * @param lastBackendStreamId the last stream ID acknowledged by the backend (in backend ID domain)
     * @return list of removed streams for cleanup
     */
    List<Stream> removeStreamsAboveBackendId(int lastBackendStreamId) {
        List<Stream> removed = new ArrayList<>();
        var iterator = map.entrySet().iterator();
        while (iterator.hasNext()) {
            Map.Entry<Integer, Stream> entry = iterator.next();
            Stream stream = entry.getValue();
            if (stream.proxyStream().id() > lastBackendStreamId) {
                removed.add(stream);
                iterator.remove();
                // V-H2-031: Also remove from the reverse map to prevent leaked entries.
                // The reverse map is keyed by backend stream ID (proxyStream().id()).
                reverseMap.remove(stream.proxyStream().id());
            }
        }
        return removed;
    }

    int size() {
        return map.size();
    }

    @Override
    public String toString() {
        return "StreamPropertyMap{map=" + map + '}';
    }

    record Stream(String acceptEncoding, Http2FrameStream clientStream, Http2FrameStream proxyStream) {
        // Simple record
    }
}
