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
package com.shieldblaze.expressgateway.protocol.http.grpc;

import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http2.DecoratingHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2Headers;

import java.util.HashSet;
import java.util.Set;

/**
 * A {@link DecoratingHttp2ConnectionEncoder} that wraps a
 * {@link io.netty.handler.codec.http2.CompressorHttp2ConnectionEncoder} and
 * bypasses its compression logic for gRPC streams.
 * <p>
 * gRPC uses its own message-level compression negotiated via the {@code grpc-encoding}
 * header. Applying HTTP/2 transport-level compression on top of that would corrupt
 * the gRPC length-prefixed framing and violate the gRPC wire protocol.
 * <p>
 * When a response HEADERS frame contains {@code content-type} starting with
 * {@code application/grpc}, this encoder delegates {@code writeHeaders} and
 * {@code writeData} directly to the raw (non-compressing) encoder, skipping the
 * compressor entirely for that stream.
 */
public final class GrpcAwareCompressorEncoder extends DecoratingHttp2ConnectionEncoder {

    private final Http2ConnectionEncoder rawEncoder;
    // All access is on the EventLoop — plain HashSet is safe and avoids fastutil dependency.
    private final Set<Integer> grpcStreamIds = new HashSet<>();

    /**
     * @param compressorEncoder the {@link io.netty.handler.codec.http2.CompressorHttp2ConnectionEncoder}
     *                          that provides compression for non-gRPC streams
     * @param rawEncoder        the underlying {@link io.netty.handler.codec.http2.DefaultHttp2ConnectionEncoder}
     *                          that writes frames without compression
     */
    public GrpcAwareCompressorEncoder(Http2ConnectionEncoder compressorEncoder, Http2ConnectionEncoder rawEncoder) {
        super(compressorEncoder);
        this.rawEncoder = rawEncoder;
    }

    @Override
    public ChannelFuture writeHeaders(ChannelHandlerContext ctx, int streamId, Http2Headers headers,
                                      int padding, boolean endStream, ChannelPromise promise) {
        if (isGrpcContentType(headers)) {
            grpcStreamIds.add(streamId);
            if (endStream) {
                grpcStreamIds.remove(streamId);
            }
            return rawEncoder.writeHeaders(ctx, streamId, headers, padding, endStream, promise);
        }
        return super.writeHeaders(ctx, streamId, headers, padding, endStream, promise);
    }

    @Override
    public ChannelFuture writeHeaders(ChannelHandlerContext ctx, int streamId, Http2Headers headers,
                                      int streamDependency, short weight, boolean exclusive,
                                      int padding, boolean endStream, ChannelPromise promise) {
        if (isGrpcContentType(headers)) {
            grpcStreamIds.add(streamId);
            if (endStream) {
                grpcStreamIds.remove(streamId);
            }
            return rawEncoder.writeHeaders(ctx, streamId, headers, streamDependency, weight,
                    exclusive, padding, endStream, promise);
        }
        return super.writeHeaders(ctx, streamId, headers, streamDependency, weight,
                exclusive, padding, endStream, promise);
    }

    @Override
    public ChannelFuture writeData(ChannelHandlerContext ctx, int streamId, ByteBuf data,
                                   int padding, boolean endStream, ChannelPromise promise) {
        if (grpcStreamIds.contains(streamId)) {
            if (endStream) {
                grpcStreamIds.remove(streamId);
            }
            return rawEncoder.writeData(ctx, streamId, data, padding, endStream, promise);
        }
        return super.writeData(ctx, streamId, data, padding, endStream, promise);
    }

    @Override
    public ChannelFuture writeRstStream(ChannelHandlerContext ctx, int streamId, long errorCode,
                                        ChannelPromise promise) {
        // Clean up gRPC tracking on stream reset to prevent unbounded growth.
        grpcStreamIds.remove(streamId);
        return super.writeRstStream(ctx, streamId, errorCode, promise);
    }

    private static boolean isGrpcContentType(Http2Headers headers) {
        // Delegate to GrpcDetector which uses zero-allocation char-by-char prefix matching
        // instead of toString().startsWith() which allocates on every H2 HEADERS write.
        return GrpcDetector.isGrpc(headers);
    }
}
