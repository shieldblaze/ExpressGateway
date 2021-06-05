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
package com.shieldblaze.expressgateway.configuration.tls;

import io.netty.handler.ssl.ClientAuth;

/**
 * Mutual TLS configuration for Server.
 * @see <a href="https://en.wikipedia.org/wiki/Mutual_authentication">Mutual TLS</a>
 */
public enum MutualTLS {
    NOT_REQUIRED(ClientAuth.NONE),
    OPTIONAL(ClientAuth.OPTIONAL),
    REQUIRED(ClientAuth.REQUIRE);

    private final ClientAuth clientAuth;

    MutualTLS(ClientAuth clientAuth) {
        this.clientAuth = clientAuth;
    }

    ClientAuth clientAuth() {
        return clientAuth;
    }
}
