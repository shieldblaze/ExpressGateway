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
package io.netty.handler.codec.http2;

/**
 * Default implementation of {@link Http2PushPromiseFrame}
 */
public class DefaultHttp2PushPromiseFrame implements Http2PushPromiseFrame {

    private Http2FrameStream pushStreamFrame;
    private final Http2Headers http2Headers;
    private Http2FrameStream streamFrame;
    private final int padding;
    private final int promisedStreamId;

    public DefaultHttp2PushPromiseFrame(Http2Headers http2Headers) {
        this(http2Headers, 0);
    }

    public DefaultHttp2PushPromiseFrame(Http2Headers http2Headers, int padding) {
        this(http2Headers, padding, -1);
    }

    DefaultHttp2PushPromiseFrame(Http2Headers http2Headers, int padding, int promisedStreamId) {
        this.http2Headers = http2Headers;
        this.padding = padding;
        this.promisedStreamId = promisedStreamId;
    }

    @Override
    public Http2StreamFrame pushStream(Http2FrameStream stream) {
        pushStreamFrame = stream;
        return this;
    }

    @Override
    public Http2FrameStream pushStream() {
        return pushStreamFrame;
    }

    @Override
    public Http2Headers http2Headers() {
        return http2Headers;
    }

    @Override
    public int padding() {
        return padding;
    }

    @Override
    public Http2StreamFrame stream(Http2FrameStream stream) {
        streamFrame = stream;
        return this;
    }

    @Override
    public Http2FrameStream stream() {
        return streamFrame;
    }

    public int getPromisedStreamId() {
        if (pushStreamFrame != null) {
            return pushStreamFrame.id();
        } else {
            return promisedStreamId;
        }
    }

    @Override
    public String name() {
        return "PUSH_PROMISE_READ_FRAME";
    }

    @Override
    public String toString() {
        return "DefaultHttp2PushPromiseFrame{" +
                "pushStreamFrame=" + pushStreamFrame +
                ", http2Headers=" + http2Headers +
                ", streamFrame=" + streamFrame +
                ", padding=" + padding +
                '}';
    }
}
