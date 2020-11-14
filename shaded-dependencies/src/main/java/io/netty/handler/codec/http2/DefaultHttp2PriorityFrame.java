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
 * Default implementation of {@linkplain Http2PriorityFrame}
 */
public class DefaultHttp2PriorityFrame implements Http2PriorityFrame {

    private final int streamDependency;
    private final short weight;
    private final boolean exclusive;
    private Http2FrameStream http2FrameStream;

    public DefaultHttp2PriorityFrame(int streamDependency, short weight, boolean exclusive) {
        this.streamDependency = streamDependency;
        this.weight = weight;
        this.exclusive = exclusive;
    }

    @Override
    public int streamDependency() {
        return streamDependency;
    }

    @Override
    public short weight() {
        return weight;
    }

    @Override
    public boolean exclusive() {
        return exclusive;
    }

    @Override
    public Http2StreamFrame stream(Http2FrameStream stream) {
        http2FrameStream = stream;
        return this;
    }

    @Override
    public Http2FrameStream stream() {
        return http2FrameStream;
    }

    @Override
    public String name() {
        return "PRIORITY_FRAME";
    }

    @Override
    public String toString() {
        return "DefaultHttp2PriorityFrame(" +
                "stream=" + http2FrameStream +
                ", streamDependency=" + streamDependency +
                ", weight=" + weight +
                ", exclusive=" + exclusive +
                ')';
    }
}
