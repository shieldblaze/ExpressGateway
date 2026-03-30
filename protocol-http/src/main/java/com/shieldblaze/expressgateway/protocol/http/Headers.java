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

package com.shieldblaze.expressgateway.protocol.http;

public final class Headers {
    public static final String X_FORWARDED_FOR = "x-forwarded-for";
    public static final String X_FORWARDED_PROTO = "x-forwarded-proto";
    public static final String X_FORWARDED_HOST = "x-forwarded-host";
    public static final String X_FORWARDED_PORT = "x-forwarded-port";
    public static final String VIA = "via";

    /**
     * Unique request identifier for end-to-end request correlation across
     * proxy, backend, and observability systems. If the client sends this
     * header, the proxy preserves it (does not overwrite). Otherwise, the
     * proxy generates a UUID v4 and injects it before forwarding.
     */
    public static final String X_REQUEST_ID = "x-request-id";

    private Headers() {
        // Prevent outside initialization
    }
}
