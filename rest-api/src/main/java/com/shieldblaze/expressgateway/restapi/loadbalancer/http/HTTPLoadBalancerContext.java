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
package com.shieldblaze.expressgateway.restapi.loadbalancer.http;

import com.fasterxml.jackson.annotation.JsonProperty;

@SuppressWarnings("unused")
final class HTTPLoadBalancerContext {

    @JsonProperty("bindAddress")
    private String bindAddress;

    @JsonProperty("bindPort")
    private int bindPort;

    @JsonProperty("algorithm")
    private String algorithm;

    @JsonProperty("sessionPersistence")
    private String sessionPersistence;

    @JsonProperty("tlsForServer")
    private boolean tlsForServer;

    @JsonProperty("tlsForClient")
    private boolean tlsForClient;

    public String bindAddress() {
        return bindAddress;
    }

    public int bindPort() {
        return bindPort;
    }

    public String algorithm() {
        return algorithm;
    }

    public String sessionPersistence() {
        return sessionPersistence;
    }

    public boolean tlsForServer() {
        return tlsForServer;
    }

    public boolean tlsForClient() {
        return tlsForClient;
    }
}