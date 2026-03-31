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

import io.netty.util.AsciiString;

/**
 * Constants for gRPC protocol handling over HTTP/2.
 */
public final class GrpcConstants {

    public static final String CONTENT_TYPE_GRPC_PREFIX = "application/grpc";

    public static final AsciiString GRPC_STATUS = AsciiString.cached("grpc-status");
    public static final AsciiString GRPC_MESSAGE = AsciiString.cached("grpc-message");
    public static final AsciiString GRPC_STATUS_DETAILS_BIN = AsciiString.cached("grpc-status-details-bin");
    public static final AsciiString GRPC_TIMEOUT = AsciiString.cached("grpc-timeout");
    public static final AsciiString GRPC_ENCODING = AsciiString.cached("grpc-encoding");
    public static final AsciiString CONTENT_TYPE = AsciiString.cached("content-type");

    // gRPC status codes (string form for direct use in Http2Headers)
    public static final String STATUS_OK = "0";
    public static final String STATUS_CANCELLED = "1";
    public static final String STATUS_DEADLINE_EXCEEDED = "4";
    public static final String STATUS_INTERNAL = "13";
    public static final String STATUS_UNAVAILABLE = "14";

    private GrpcConstants() {
        // Prevent outside initialization
    }
}
