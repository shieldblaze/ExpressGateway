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

import com.shieldblaze.expressgateway.backend.connection.Connection;
import io.netty.handler.codec.http2.Http2FrameStream;

final class Property {

    private Http2FrameStream upstreamFrameStream;
    private Http2FrameStream downstreamFrameStream;
    private String acceptEncoding;
    private Connection connection;

    public Http2FrameStream getUpstreamFrameStream() {
        return upstreamFrameStream;
    }

    public void setUpstreamFrameStream(Http2FrameStream upstreamFrameStream) {
        this.upstreamFrameStream = upstreamFrameStream;
    }

    public Http2FrameStream getDownstreamFrameStream() {
        return downstreamFrameStream;
    }

    public void setDownstreamFrameStream(Http2FrameStream downstreamFrameStream) {
        this.downstreamFrameStream = downstreamFrameStream;
    }

    public String getAcceptEncoding() {
        return acceptEncoding;
    }

    public void setAcceptEncoding(String acceptEncoding) {
        this.acceptEncoding = acceptEncoding;
    }

    public Connection getConnection() {
        return connection;
    }

    public void setConnection(Connection connection) {
        this.connection = connection;
    }
}
