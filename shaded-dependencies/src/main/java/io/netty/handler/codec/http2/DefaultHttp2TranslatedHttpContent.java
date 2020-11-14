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

import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.http.HttpContent;
import io.netty.util.internal.StringUtil;

/**
 * {@link DefaultHttp2TranslatedHttpContent} contains {@link HttpContent} and {@code streamId}
 * which are translated from {@link Http2DataFrame}
 */
public class DefaultHttp2TranslatedHttpContent extends Http2TranslatedHttpContent {

    /**
     * Creates a new instance with the specified chunk content and StreamId.
     */
    public DefaultHttp2TranslatedHttpContent(ByteBuf content, long streamId) {
        super(content, streamId);
    }

    @Override
    public String toString() {
        return StringUtil.simpleClassName(this) + "(streamId: " + streamId() + ", data: " + content() +
                ", decoderResult: " + decoderResult() + ')';
    }
}
