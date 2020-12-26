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
package com.shieldblaze.expressgateway.restapi;

import com.fasterxml.jackson.annotation.JsonProperty;

final class Transport {

    @JsonProperty("type")
    private String type;

    @JsonProperty("tcpFastOpenPendingRequests")
    private int tcpFastOpenPendingRequests;

    @JsonProperty("backendConnectTimeout")
    private int backendConnectTimeout;

    @JsonProperty("receiveBufferAllocationType")
    private String receiveBufferAllocationType;

    @JsonProperty("receiveBufferSizes")
    private int[] receiveBufferSizes;

    @JsonProperty("socketReceiveBufferSize")
    private int socketReceiveBufferSize;

    @JsonProperty("socketSendBufferSize")
    private int socketSendBufferSize;

    @JsonProperty("tcpConnectionBacklog")
    private int tcpConnectionBacklog;

    @JsonProperty("connectionIdleTimeout")
    private int connectionIdleTimeout;

    public String type() {
        return type;
    }

    public int tcpFastOpenPendingRequests() {
        return tcpFastOpenPendingRequests;
    }

    public int backendConnectTimeout() {
        return backendConnectTimeout;
    }

    public String receiveBufferAllocationType() {
        return receiveBufferAllocationType;
    }

    public int[] receiveBufferSizes() {
        return receiveBufferSizes;
    }

    public int socketReceiveBufferSize() {
        return socketReceiveBufferSize;
    }

    public int socketSendBufferSize() {
        return socketSendBufferSize;
    }

    public int tcpConnectionBacklog() {
        return tcpConnectionBacklog;
    }

    public int connectionIdleTimeout() {
        return connectionIdleTimeout;
    }
}
