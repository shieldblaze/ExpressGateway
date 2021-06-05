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

public interface HttpFrame {

    /**
     * Returns the actual protocol used in this Http Frame
     */
    Protocol protocol();

    /**
     * Set the actual protocol used in this Http Frame
     */
    void protocol(Protocol protocol);

    /**
     * Returns a globally unique identifier for this Http Frame
     */
    long id();

    /**
     * Set a globally unique identifier for this Http Frame
     */
    void id(long id);

    enum Protocol {
        /**
         * HTTP/2 over TLS
         */
        H2,
        /**
         * HTTP 1.1
         */
        HTTP_1_1,
        /**
         * HTTP 1.0
         */
        HTTP_1_0
    }
}
