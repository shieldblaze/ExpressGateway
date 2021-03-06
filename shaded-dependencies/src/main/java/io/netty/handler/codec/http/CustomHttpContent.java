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

public class CustomHttpContent extends DefaultHttpContent implements HttpFrame{

    private Protocol protocol;
    private long id;

    public CustomHttpContent(ByteBuf content, Protocol protocol, long id) {
        super(content);
        this.protocol = protocol;
        this.id = id;
    }

    @Override
    public String toString() {
        return "CustomHttpContent{" +
                "protocol=" + protocol +
                ", id=" + id +
                '}';
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
