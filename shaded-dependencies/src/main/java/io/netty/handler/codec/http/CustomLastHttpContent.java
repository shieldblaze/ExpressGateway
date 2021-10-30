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
package io.netty.handler.codec.http;

import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.DefaultHeaders;

public class CustomLastHttpContent extends CustomHttpContent implements LastHttpContent {

    private final HttpHeaders trailingHeaders;

    public CustomLastHttpContent(ByteBuf content, Protocol protocol, long id) {
        super(content, protocol, id);
        this.trailingHeaders = new TrailingHttpHeaders(true);
    }

    @Override
    public LastHttpContent copy() {
        return replace(content().copy());
    }

    @Override
    public LastHttpContent duplicate() {
        return replace(content().duplicate());
    }

    @Override
    public LastHttpContent retainedDuplicate() {
        return replace(content().retainedDuplicate());
    }

    @Override
    public LastHttpContent replace(ByteBuf content) {
        final CustomLastHttpContent dup = new CustomLastHttpContent(content, protocol(), id());
        dup.trailingHeaders().set(trailingHeaders());
        return dup;
    }

    @Override
    public LastHttpContent retain(int increment) {
        super.retain(increment);
        return this;
    }

    @Override
    public LastHttpContent retain() {
        super.retain();
        return this;
    }

    @Override
    public LastHttpContent touch() {
        super.touch();
        return this;
    }

    @Override
    public LastHttpContent touch(Object hint) {
        super.touch(hint);
        return this;
    }

    @Override
    public HttpHeaders trailingHeaders() {
        return trailingHeaders;
    }

    @Override
    public String toString() {
        return "CustomLastHttpContent{" +
                "protocol=" + protocol() +
                ", id=" + id() +
                '}';
    }

    private static final class TrailingHttpHeaders extends DefaultHttpHeaders {
        private static final DefaultHeaders.NameValidator<CharSequence> TrailerNameValidator = new DefaultHeaders.NameValidator<CharSequence>() {
            @Override
            public void validateName(CharSequence name) {
                DefaultHttpHeaders.HttpNameValidator.validateName(name);
                if (HttpHeaderNames.CONTENT_LENGTH.contentEqualsIgnoreCase(name)
                        || HttpHeaderNames.TRANSFER_ENCODING.contentEqualsIgnoreCase(name)
                        || HttpHeaderNames.TRAILER.contentEqualsIgnoreCase(name)) {
                    throw new IllegalArgumentException("prohibited trailing header: " + name);
                }
            }
        };

        @SuppressWarnings({"unchecked"})
        TrailingHttpHeaders(boolean validate) {
            super(validate, validate ? TrailerNameValidator : DefaultHeaders.NameValidator.NOT_NULL);
        }
    }
}
