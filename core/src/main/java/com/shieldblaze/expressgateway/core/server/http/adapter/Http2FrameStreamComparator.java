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

import io.netty.handler.codec.http2.Http2FrameStream;

import java.util.Comparator;

/**
 * {@link Comparator} implementation for comparing {@link Http2FrameStream}
 * using {@link Http2FrameStream#id()}.
 */
final class Http2FrameStreamComparator implements Comparator<Http2FrameStream> {

    final static Http2FrameStreamComparator INSTANCE = new Http2FrameStreamComparator();

    private Http2FrameStreamComparator() {
        // Prevent outside initialization
    }

    @Override
    public int compare(Http2FrameStream o1, Http2FrameStream o2) {
        return Integer.compare(o1.id(), o2.id());
    }
}
