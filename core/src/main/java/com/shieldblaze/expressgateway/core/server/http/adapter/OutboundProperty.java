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
package com.shieldblaze.expressgateway.core.server.http.adapter;

import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http2.Http2CodecUtil;
import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.HttpConversionUtil;

final class OutboundProperty {
    private final String scheme;
    private final int streamId;
    private final int dependencyId;
    private final short streamWeight;
    private final Http2FrameStream http2FrameStream;
    private boolean isInitialRead;

    OutboundProperty(HttpHeaders httpHeaders, Http2FrameStream http2FrameStream) {
        scheme = httpHeaders.get(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text());
        streamId = httpHeaders.getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text());
        dependencyId = httpHeaders.getInt(HttpConversionUtil.ExtensionHeaderNames.STREAM_DEPENDENCY_ID.text(), 0);
        streamWeight = httpHeaders.getShort(HttpConversionUtil.ExtensionHeaderNames.STREAM_WEIGHT.text(), Http2CodecUtil.DEFAULT_PRIORITY_WEIGHT);
        this.http2FrameStream = http2FrameStream;
    }

    String getScheme() {
        return scheme;
    }

    int getStreamId() {
        return streamId;
    }

    int getDependencyId() {
        return dependencyId;
    }

    short getStreamWeight() {
        return streamWeight;
    }

    Http2FrameStream getHttp2FrameStream() {
        return http2FrameStream;
    }

    boolean isInitialRead() {
        return isInitialRead;
    }

    void fireInitialRead() {
        isInitialRead = true;
    }
}
