/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
import io.netty.buffer.ByteBufUtil;
import io.netty.util.IllegalReferenceCountException;

public class CustomFullHttpRequest extends CustomHttpRequest implements FullHttpRequest {

    private final ByteBuf content;
    private final HttpHeaders trailingHeader;

    /**
     * Used to cache the value of the hash code and avoid {@link IllegalReferenceCountException}.
     */
    private int hash;

    public CustomFullHttpRequest(HttpVersion httpVersion, HttpMethod method, String uri, ByteBuf content, Protocol protocol, long id) {
        super(httpVersion, method, uri, protocol, id);
        this.content = content;
        trailingHeader = new DefaultHttpHeaders(true);
    }

    @Override
    public HttpHeaders trailingHeaders() {
        return trailingHeader;
    }

    @Override
    public ByteBuf content() {
        return content;
    }

    @Override
    public int refCnt() {
        return content.refCnt();
    }

    @Override
    public FullHttpRequest retain() {
        content.retain();
        return this;
    }

    @Override
    public FullHttpRequest retain(int increment) {
        content.retain(increment);
        return this;
    }

    @Override
    public FullHttpRequest touch() {
        content.touch();
        return this;
    }

    @Override
    public FullHttpRequest touch(Object hint) {
        content.touch(hint);
        return this;
    }

    @Override
    public boolean release() {
        return content.release();
    }

    @Override
    public boolean release(int decrement) {
        return content.release(decrement);
    }

    @Override
    public FullHttpRequest setProtocolVersion(HttpVersion version) {
        super.setProtocolVersion(version);
        return this;
    }

    @Override
    public FullHttpRequest setMethod(HttpMethod method) {
        super.setMethod(method);
        return this;
    }

    @Override
    public FullHttpRequest setUri(String uri) {
        super.setUri(uri);
        return this;
    }

    @Override
    public FullHttpRequest copy() {
        return replace(content().copy());
    }

    @Override
    public FullHttpRequest duplicate() {
        return replace(content().duplicate());
    }

    @Override
    public FullHttpRequest retainedDuplicate() {
        return replace(content().retainedDuplicate());
    }

    @Override
    public FullHttpRequest replace(ByteBuf content) {
        FullHttpRequest request = new DefaultFullHttpRequest(protocolVersion(), method(), uri(), content,
                headers().copy(), trailingHeaders().copy());
        request.setDecoderResult(decoderResult());
        return request;
    }

    @Override
    public int hashCode() {
        int hash = this.hash;
        if (hash == 0) {
            if (ByteBufUtil.isAccessible(content())) {
                try {
                    hash = 31 + content().hashCode();
                } catch (IllegalReferenceCountException ignored) {
                    // Handle race condition between checking refCnt() == 0 and using the object.
                    hash = 31;
                }
            } else {
                hash = 31;
            }
            hash = 31 * hash + trailingHeaders().hashCode();
            hash = 31 * hash + super.hashCode();
            this.hash = hash;
        }
        return hash;
    }

    @Override
    public boolean equals(Object o) {
        if (!(o instanceof CustomFullHttpRequest)) {
            return false;
        }

        CustomFullHttpRequest other = (CustomFullHttpRequest) o;

        return super.equals(other) &&
                content().equals(other.content()) &&
                trailingHeaders().equals(other.trailingHeaders());
    }

    @Override
    public String toString() {
        return HttpMessageUtil.appendFullRequest(new StringBuilder(256), this).toString() + "/" + id();
    }
}
