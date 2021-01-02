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
package com.shieldblaze.expressgateway.restapi.loadbalancer.l4;

import com.fasterxml.jackson.annotation.JsonProperty;

@SuppressWarnings("unused")
final class L4LoadBalancerContext {

    @JsonProperty("bindAddress")
    private String bindAddress;

    @JsonProperty("bindPort")
    private int bindPort;

    @JsonProperty("protocol")
    private String protocol;

    @JsonProperty("algorithm")
    private String algorithm;

    @JsonProperty("sessionPersistence")
    private String sessionPersistence;

    @JsonProperty("tlsForServer")
    private boolean tlsForServer;

    @JsonProperty("tlsForClient")
    private boolean tlsForClient;

    String bindAddress() {
        return bindAddress;
    }

    int bindPort() {
        return bindPort;
    }

    String protocol() {
        return protocol;
    }

    String algorithm() {
        return algorithm;
    }

    String sessionPersistence() {
        return sessionPersistence;
    }

    boolean tlsForServer() {
        return tlsForServer;
    }

    boolean tlsForClient() {
        return tlsForClient;
    }
}
