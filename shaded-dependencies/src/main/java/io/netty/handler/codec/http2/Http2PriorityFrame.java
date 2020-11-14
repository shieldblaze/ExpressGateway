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

import io.netty.handler.codec.http2.Http2StreamFrame;

/**
 * HTTP/2 Priority Frame
 */
public interface Http2PriorityFrame extends Http2StreamFrame {

    /**
     * Parent Stream Id of this Priority request
     */
    int streamDependency();

    /**
     * Stream weight
     */
    short weight();

    /**
     * Set to {@code true} if this stream is exclusive else set to {@code false}
     */
    boolean exclusive();
}
