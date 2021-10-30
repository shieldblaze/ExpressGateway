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

public class CustomHttpResponse extends DefaultHttpResponse implements HttpFrame {

    private Protocol protocol;
    private long id;

    public CustomHttpResponse(HttpVersion httpVersion, HttpResponseStatus status, Protocol protocol, long id) {
        super(httpVersion, status, true);
        this.protocol = protocol;
        this.id = id;
    }

    @Override
    public String toString() {
        return HttpMessageUtil.appendResponse(new StringBuilder(256), this) +  "/" + id();
    }

    @Override
    public Protocol protocol() {
        return protocol;
    }

    @Override
    public void protocol(Protocol protocol) {
        this.protocol = protocol;
    }

    @Override
    public long id() {
        return id;
    }

    @Override
    public void id(long id) {
        this.id = id;
    }
}
