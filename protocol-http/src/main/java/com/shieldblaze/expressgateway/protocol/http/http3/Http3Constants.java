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
package com.shieldblaze.expressgateway.protocol.http.http3;

import io.netty.util.AttributeKey;

/**
 * Shared constants for HTTP/3 protocol handling.
 */
public final class Http3Constants {

    private Http3Constants() {
    }

    /**
     * AttributeKey set on the parent QuicChannel when the connection is draining.
     * Used by Http3ServerHandler to reject new streams with 503 during graceful shutdown.
     *
     * @see <a href="https://www.rfc-editor.org/rfc/rfc9114.html#section-5.2">RFC 9114 Section 5.2</a>
     */
    public static final AttributeKey<Boolean> DRAINING_KEY = AttributeKey.valueOf("h3.draining");
}
