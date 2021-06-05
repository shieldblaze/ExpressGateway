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
package com.shieldblaze.expressgateway.restapi.api.loadbalancer;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.common.utils.NumberUtil;

import java.util.Objects;

public final class LoadBalancerStartStruct {

    @JsonProperty("name")
    private String name;

    @JsonProperty("bindAddress")
    private String bindAddress;

    @JsonProperty("bindPort")
    private int bindPort;

    @JsonProperty("protocol")
    private String protocol;

    @JsonProperty("tlsForServer")
    private boolean tlsForServer;

    @JsonProperty("tlsForClient")
    private boolean tlsForClient;

    public void setName(String name) {
        this.name = name;
    }

    public void setBindAddress(String bindAddress) {
        this.bindAddress = Objects.requireNonNull(bindAddress, "BindAddress");
    }

    public void setBindPort(int bindPort) {
        this.bindPort = NumberUtil.checkRange(bindPort, 1, 65535, "BindPort");
    }

    public void setProtocol(String protocol) {
        this.protocol = protocol;
    }

    public void setTlsForServer(boolean tlsForServer) {
        this.tlsForServer = tlsForServer;
    }

    public void setTlsForClient(boolean tlsForClient) {
        this.tlsForClient = tlsForClient;
    }

    public String name() {
        return name;
    }

    public String bindAddress() {
        return bindAddress;
    }

    public int bindPort() {
        return bindPort;
    }

    public String protocol() {
        return protocol;
    }

    public boolean tlsForServer() {
        return tlsForServer;
    }

    public boolean tlsForClient() {
        return tlsForClient;
    }
}
