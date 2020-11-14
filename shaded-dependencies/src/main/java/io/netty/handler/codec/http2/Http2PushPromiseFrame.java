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

import io.netty.handler.codec.http2.Http2FrameStream;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2StreamFrame;

/**
 * HTTP/2 Push Promise Frame
 */
public interface Http2PushPromiseFrame extends Http2StreamFrame {

    /**
     * Set the Promise {@link Http2FrameStream} object for this frame.
     */
    Http2StreamFrame pushStream(Http2FrameStream stream);

    /**
     * Returns the Promise {@link Http2FrameStream} object for this frame, or {@code null} if the
     * frame has yet to be associated with a stream.
     */
    Http2FrameStream pushStream();

    /**
     * {@link Http2Headers} sent in Push Promise
     */
    Http2Headers http2Headers();

    /**
     * Frame padding to use. Will be non-negative and less than 256.
     */
    int padding();
}
