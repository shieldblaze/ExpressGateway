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

import io.netty.handler.codec.http2.Http2Headers;

/**
 * Detects gRPC requests from HTTP/2 headers.
 */
public final class GrpcDetector {

    /**
     * Returns {@code true} if the headers indicate a gRPC request
     * (content-type starts with "application/grpc").
     */
    private static final int PREFIX_LEN = GrpcConstants.CONTENT_TYPE_GRPC_PREFIX.length();

    public static boolean isGrpc(Http2Headers headers) {
        CharSequence contentType = headers.get(GrpcConstants.CONTENT_TYPE);
        if (contentType == null || contentType.length() < PREFIX_LEN) {
            return false;
        }
        // Zero-allocation prefix check — avoids toString()/AsciiString.of() on hot path.
        // "application/grpc" is ASCII-only, so char-by-char comparison is safe.
        for (int i = 0; i < PREFIX_LEN; i++) {
            if (Character.toLowerCase(contentType.charAt(i)) != GrpcConstants.CONTENT_TYPE_GRPC_PREFIX.charAt(i)) {
                return false;
            }
        }
        return true;
    }

    /**
     * Returns {@code true} if the request targets the standard gRPC health check endpoint.
     */
    public static boolean isGrpcHealthCheck(Http2Headers headers) {
        CharSequence path = headers.path();
        return path != null && "/grpc.health.v1.Health/Check".contentEquals(path);
    }

    private GrpcDetector() {
        // Prevent outside initialization
    }
}
